<script lang="ts">
  // The Markdown toolbar of every Markdown editor (text blocks, notes, review comments). Its
  // buttons keep the editor's focus and selection; the command is applied by the editor
  // (lib/markdown-monaco.ts `applyMarkdownCommand`).
  import { Bold, Code, FileCode, Heading, Heading1, Heading2, Heading3, Italic, Link, List, ListOrdered, Quote, Sigma, SquareSigma } from "@lucide/svelte";
  import type { MarkdownCommand } from "../lib/markdown-commands";
  let { onCommand, disabled = false }: { onCommand: (command: MarkdownCommand) => void; disabled?: boolean } = $props();
  let headings = $state(false);
  const mac = /Mac|iPhone|iPad/.test(navigator.platform);
  const key = (k: string) => (mac ? `⌘${k}` : `Ctrl+${k}`);
  const groups: { command: MarkdownCommand; label: string; icon: typeof Bold }[][] = [
    [
      { command: "bold", label: `Bold (${key("B")})`, icon: Bold },
      { command: "italic", label: `Italic (${key("I")})`, icon: Italic },
      { command: "code", label: "Inline code", icon: Code },
    ],
    [
      { command: "math", label: "Inline math $…$", icon: Sigma },
      { command: "displayMath", label: "Display math $$…$$", icon: SquareSigma },
      { command: "codeBlock", label: "Lean code block", icon: FileCode },
    ],
    [
      { command: "bulletList", label: "Bulleted list", icon: List },
      { command: "numberedList", label: "Numbered list", icon: ListOrdered },
      { command: "quote", label: "Quote", icon: Quote },
      { command: "link", label: "Link", icon: Link },
    ],
  ];
  const levels = [
    { command: "heading1" as const, label: "Heading 1", icon: Heading1 },
    { command: "heading2" as const, label: "Heading 2", icon: Heading2 },
    { command: "heading3" as const, label: "Heading 3", icon: Heading3 },
  ];
  function run(command: MarkdownCommand) {
    headings = false;
    onCommand(command);
  }
</script>

<!-- mousedown is prevented so that the editor keeps its focus and selection. -->
<div class="markdown-toolbar" role="toolbar" aria-label="Markdown formatting">
  <span class="menu">
    <button type="button" {disabled} title="Heading" aria-label="Heading" aria-haspopup="menu" aria-expanded={headings}
      onmousedown={(e) => e.preventDefault()} onclick={() => (headings = !headings)}><Heading size={15} strokeWidth={1.8} /></button>
    {#if headings}
      <span class="popup" role="menu">
        {#each levels as l (l.command)}
          <button type="button" role="menuitem" title={l.label} aria-label={l.label} onmousedown={(e) => e.preventDefault()} onclick={() => run(l.command)}><l.icon size={15} strokeWidth={1.8} /></button>
        {/each}
      </span>
    {/if}
  </span>
  {#each groups as group, i (i)}
    <span class="sep" aria-hidden="true"></span>
    {#each group as b (b.command)}
      <button type="button" {disabled} title={b.label} aria-label={b.label} onmousedown={(e) => e.preventDefault()} onclick={() => run(b.command)}><b.icon size={15} strokeWidth={1.8} /></button>
    {/each}
  {/each}
</div>

<style>
  .markdown-toolbar { display: flex; flex-wrap: wrap; align-items: center; gap: 1px; }
  .markdown-toolbar button { display: grid; place-items: center; width: 25px; height: 25px; padding: 0; border: none; border-radius: var(--mdc-radius-sm); background: none; color: var(--mdc-dim); cursor: pointer; }
  .markdown-toolbar button:hover:not(:disabled) { background: var(--mdc-card-hover); color: var(--mdc-fg); }
  .markdown-toolbar button:disabled { opacity: .4; cursor: default; }
  .sep { width: 1px; height: 16px; margin: 0 3px; background: var(--mdc-border); }
  .menu { position: relative; }
  .popup { position: absolute; z-index: 20; top: 100%; left: 0; display: flex; gap: 1px; margin-top: 2px; padding: 2px; border: 1px solid var(--mdc-border); border-radius: var(--mdc-radius-sm); background: var(--mdc-panel-raised); box-shadow: var(--mdc-shadow-lg); }
</style>
