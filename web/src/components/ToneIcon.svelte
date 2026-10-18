<svelte:options namespace="svg" />

<script lang="ts">
  // A node's Lean standing as an icon centred on the origin (lib/node-tone.ts): the graph's
  // cards and its legend draw the same one.
  import { isDefinitionTone, type NodeTone } from "../lib/node-tone";
  let { tone }: { tone: NodeTone } = $props();
  const filled = $derived(tone === "registered" || tone === "derivable" || tone === "certified" || tone === "problem");
</script>

<g class="tone-icon" data-tone={tone}>
  {#if isDefinitionTone(tone)}
    <rect x="-4.4" y="-4.4" width="8.8" height="8.8" rx="1.4" transform="rotate(45)" class:filled />
  {:else}
    <circle r={filled ? 5.5 : 5} class:filled class:dashed={tone === "no_code"} />
    {#if tone === "certified"}<path d="M-2.6 0 L-0.6 2 L2.8 -1.8" />{/if}
    {#if tone === "problem"}<path d="M0 -2.6 L0 0.6 M0 2.6 L0 2.7" />{/if}
  {/if}
</g>

<style>
  .tone-icon { --tone: var(--mdc-muted); }
  .tone-icon[data-tone="insufficient"] { --tone: var(--mdc-warning); }
  .tone-icon[data-tone="derivable"], .tone-icon[data-tone="imported"] { --tone: var(--mdc-accent); }
  .tone-icon[data-tone="certified"], .tone-icon[data-tone="registered"] { --tone: var(--mdc-accent-down); }
  .tone-icon[data-tone="problem"] { --tone: var(--mdc-error); }
  rect, circle { fill: none; stroke: var(--tone); stroke-width: 1.6; }
  .filled { fill: var(--tone); }
  .dashed { stroke-dasharray: 2.2 2; }
  path { fill: none; stroke: var(--mdc-panel); stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; }
</style>
