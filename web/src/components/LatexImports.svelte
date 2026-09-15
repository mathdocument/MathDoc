<script lang="ts">
  import type { LatexImport } from '../lib/latex';
  let { imports }: { imports: LatexImport[] | null } = $props();
  let query = $state('');
  let matches = $derived(imports?.filter(item => `${item.title} ${item.fnode}`.toLowerCase().includes(query.toLowerCase())) ?? []);
</script>
<details class="latex-imports">
  <summary>{imports ? `${imports.length} imported dependencies` : 'Loading reference scope…'}<span>read-only</span></summary>
  <div class="imports-body">
    <p>References are available from these nodes. Change dependencies to change this list.</p>
    {#if (imports?.length ?? 0) > 20}<input aria-label="Filter LaTeX imports" placeholder="Filter dependencies…" bind:value={query} />{/if}
    <ul>
      {#each matches.slice(0, 50) as item (item.fnode)}
        <li><span title={item.title}>{item.title}</span><code title="Reference prefix">{item.prefix}</code></li>
      {/each}
    </ul>
    {#if matches.length > 50}<p>Showing 50 of {matches.length}. Refine the filter to see more.</p>{/if}
  </div>
</details>
<style>
  .latex-imports { border-bottom:1px solid var(--mdc-border); background:var(--mdc-code-bg); color:var(--mdc-muted); font-size:var(--mdc-text-xs); }
  summary { padding:.55rem .75rem; cursor:pointer; }
  summary span { float:right; opacity:.65; }
  .imports-body { padding:0 .75rem .55rem; }
  p { margin:.25rem 0 .6rem; }
  ul { list-style:none; margin:0; padding:0; }
  li { padding:.35rem 0; border-top:1px solid var(--mdc-border); }
  li span { display:block; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; color:var(--mdc-fg); }
  code { font-size:.7rem; overflow-wrap:anywhere; }
  input { width:100%; padding:.4rem; margin-bottom:.5rem; background:var(--mdc-bg); color:var(--mdc-fg); border:1px solid var(--mdc-border); }
</style>
