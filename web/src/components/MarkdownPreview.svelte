<script lang="ts">
  import { renderMath } from '../lib/latex-math';
  import { renderMarkdown } from '../lib/markdown';
  import { chainEditorScroll } from '../lib/editor-scroll';

  // compact: inline in a dialog, e.g. a decomposition's description; no pane padding or scroll chaining.
  let { source, compact = false }: { source: string; compact?: boolean } = $props();
  let host = $state<HTMLDivElement>();
  const html = $derived(renderMarkdown(source));
  $effect(() => {
    void html;
    if (host) return renderMath(host);
  });
</script>

<!-- renderMarkdown sanitizes the HTML a text block may contain. -->
{#if compact}
  <div class="markdown-preview compact" bind:this={host}>{@html html}</div>
{:else}
  <div class="markdown-preview" bind:this={host} use:chainEditorScroll>{@html html}</div>
{/if}

<style>
  .markdown-preview { min-height:0; overflow:auto; padding:1rem 1.4rem; color:var(--mdc-fg); font-family:var(--mdc-font); font-size:.9375rem; line-height:1.7; overflow-wrap:anywhere; }
  .markdown-preview > :global(:first-child) { margin-top:0; }
  .markdown-preview > :global(:last-child) { margin-bottom:0; }
  .markdown-preview :global(:is(p, ul, ol, blockquote, pre, table)) { margin:.6em 0; }
  .markdown-preview :global(:is(h1, h2, h3, h4, h5, h6)) { margin:1.1em 0 .5em; line-height:1.35; }
  .markdown-preview :global(h1) { font-size:1.5em; }
  .markdown-preview :global(h2) { font-size:1.3em; }
  .markdown-preview :global(h3) { font-size:1.12em; }
  .markdown-preview :global(:is(h4, h5, h6)) { font-size:1em; }
  .markdown-preview :global(:is(ul, ol)) { padding-left:1.6em; }
  .markdown-preview :global(li > :is(ul, ol)) { margin:.2em 0; }
  .markdown-preview :global(a[href]) { color:var(--mdc-accent); text-decoration:underline; text-underline-offset:3px; }
  .markdown-preview :global(blockquote) { padding:0 .9em; color:var(--mdc-fg-soft); border-left:2px solid var(--mdc-border-strong); }
  .markdown-preview :global(:is(pre, code)) { font-family:var(--mdc-mono); font-size:.85em; }
  .markdown-preview :global(:not(pre) > code) { padding:.1em .35em; border-radius:4px; background:var(--mdc-panel-raised); }
  .markdown-preview :global(pre) { overflow-x:auto; padding:.7em .9em; border-radius:6px; background:var(--mdc-panel-raised); }
  .markdown-preview :global(table) { display:block; max-width:100%; overflow-x:auto; border-collapse:collapse; }
  .markdown-preview :global(:is(th, td)) { padding:.3em .7em; border:1px solid var(--mdc-border-strong); }
  .markdown-preview :global(hr) { border:0; border-top:1px solid var(--mdc-border-strong); margin:1.2em 0; }
  .markdown-preview :global(img) { max-width:100%; }
  .markdown-preview :global(.latex-math[data-display="true"]) { display:block; overflow-x:auto; margin:.6em 0; font-size:1.1em; }
  .markdown-preview :global(.latex-math[aria-busy="true"]) { visibility:hidden; }
  .markdown-preview :global(.latex-error) { color:var(--mdc-error); }
  .markdown-preview.compact { overflow:visible; padding:0; font-size:inherit; line-height:1.6; }
  .markdown-preview.compact :global(:is(p, ul, ol, blockquote, pre, table)) { margin:.3em 0; }
</style>
