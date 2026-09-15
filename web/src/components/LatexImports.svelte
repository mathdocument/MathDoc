<script lang="ts">
  import type { LatexImport } from '../lib/latex';
  let { imports }: { imports: LatexImport[] | null } = $props();
  let query = $state('');
  let matches = $derived(imports?.filter(item => `${item.title} ${item.fnode}`.toLowerCase().includes(query.toLowerCase())) ?? []);
</script>
<details class="latex-imports">
  <summary>{imports ? `${imports.length} imported dependencies` : 'Loading reference scope…'}</summary>
  <div class="imports-body">
    {#if (imports?.length ?? 0) > 20}<input aria-label="Filter LaTeX imports" placeholder="Filter dependencies…" bind:value={query} />{/if}
    <ul>
      {#each matches.slice(0, 50) as item (item.fnode)}
        <li title={`${item.fnode} ${item.title}`}><code>{item.fnode.slice(0, 8)}</code> <span>{item.title}</span></li>
      {/each}
    </ul>
    {#if matches.length > 50}<p>Showing 50 of {matches.length}. Refine the filter to see more.</p>{/if}
  </div>
</details>
<style>
  .latex-imports { flex-shrink:0; max-height:30cqh; overflow:auto; overscroll-behavior:contain; border-bottom:1px solid var(--mdc-border); background:var(--mdc-code-bg); color:var(--mdc-muted); font-size:var(--mdc-text-xs); }
  summary { padding:.55rem .75rem; cursor:pointer; }
  .imports-body { padding:0 .75rem .55rem; }
  p { margin:.25rem 0 .6rem; }
  ul { list-style:none; margin:0; padding:0; }
  li { display:flex; align-items:baseline; gap:1ch; padding:.35rem 0; border-top:1px solid var(--mdc-border); }
  li span { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; color:var(--mdc-fg); }
  code { flex-shrink:0; font-family:var(--mdc-mono); font-size:inherit; }
  input { width:100%; padding:.4rem; margin-bottom:.5rem; background:var(--mdc-bg); color:var(--mdc-fg); border:1px solid var(--mdc-border); }
</style>
