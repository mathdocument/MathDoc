<script lang="ts">
  // A certificate's definitions and Lean source, read from LeanGround when first shown.
  // Definitions registered with a sketch are no longer in its source, so they come separately.
  import { untrack } from "svelte";
  import type { Collab } from "../../lib/collab.svelte";
  let { c, cert }: { c: Collab; cert: string } = $props();
  $effect(() => {
    const id = cert;
    untrack(() => void c.loadDetail(id));
  });
  const d = $derived(c.details[cert]);
  /** A module certificate's source is its files as JSON (docs/mathdoc.md §4.13). */
  function filesOf(source: string): [string, string][] | null {
    try {
      const v = JSON.parse(source);
      if (!v || typeof v !== "object" || Array.isArray(v)) return null;
      const entries = Object.entries(v);
      return entries.length && entries.every(([p, t]) => p.endsWith(".lean") && typeof t === "string")
        ? (entries as [string, string][])
        : null;
    } catch { return null; }
  }
  const files = $derived(d && d !== "loading" && !("error" in d) && d.source ? filesOf(d.source) : null);
</script>

{#if !d || d === "loading"}
  <p class="muted">Reading the certificate from LeanGround…</p>
{:else if "error" in d}
  <p class="error">{d.error}</p>
{:else}
  {#if d.definition_sources.length}
    <p class="group">Definitions used</p>
    {#each d.definition_sources as def (def.id)}<pre class="lean" title={def.id}>{def.source.trim()}</pre>{/each}
  {/if}
  {#if files}
    {#each files as [path, text] (path)}
      <details class="source"><summary>{path}</summary><pre class="lean">{text.trim()}</pre></details>
    {/each}
  {:else if d.source}
    <details class="source"><summary>Lean source</summary><pre class="lean">{d.source.trim()}</pre></details>
  {/if}
{/if}
