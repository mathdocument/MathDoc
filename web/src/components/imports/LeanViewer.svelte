<script lang="ts">
  // A read-only Lean file: Monaco with its own scrolling, line numbers and search, highlighted
  // like the editor's Lean blocks. The text shows as plain preformatted text until Monaco is ready.
  import { onDestroy, onMount } from "svelte";
  import type { editor } from "monaco-editor";
  import { currentTheme, observeTheme } from "../../lib/theme";
  let { text, name, line = 1 }: { text: string; name: string; line?: number } = $props();

  let host = $state<HTMLDivElement>();
  let ready = $state(false);
  let view: editor.IStandaloneCodeEditor | undefined;
  let model: editor.ITextModel | undefined;
  let alive = true;
  let stopTheme: (() => void) | undefined;

  onMount(async () => {
    const [{ editor: Monaco, Uri }, monaco] = await Promise.all([import("monaco-editor"), import("../../lib/monaco")]);
    const language = await monaco.loadSourceLanguage("lean");
    await monaco.setMonacoTheme(currentTheme());
    if (!alive || !host) return;
    model = Monaco.createModel(text, language, Uri.parse(`inmemory://mdc/import/${encodeURIComponent(name)}.lean`));
    view = Monaco.create(host, {
      ...monaco.sourceOptions,
      model,
      readOnly: true,
      domReadOnly: true,
      automaticLayout: true,
      wordWrap: "off",
      lineNumbersMinChars: 4,
      folding: true,
      renderLineHighlight: "line",
      scrollbar: { vertical: "auto", horizontal: "auto", handleMouseWheel: true },
      ariaLabel: `${name} (read only)`,
    });
    view.revealLineInCenter(line);
    stopTheme = observeTheme((next) => void monaco.setMonacoTheme(next));
    requestAnimationFrame(() => {
      if (!alive || !view) return;
      monaco.renderSource(view);
      ready = true;
    });
  });
  // A new file replaces the model's text; the editor stays.
  $effect(() => {
    const next = text;
    if (model && model.getValue() !== next) {
      model.setValue(next);
      view?.setScrollTop(0);
    }
  });
  $effect(() => {
    const target = line;
    if (view && target > 1) view.revealLineInCenter(target);
  });
  onDestroy(() => {
    alive = false;
    stopTheme?.();
    view?.dispose();
    model?.dispose();
  });
</script>

<div class="lean-viewer">
  {#if !ready}<pre class="lean fallback">{text}</pre>{/if}
  <div class="host" class:hidden={!ready} bind:this={host}></div>
</div>

<style>
  .lean-viewer { position: relative; height: 100%; min-height: 12rem; border: 1px solid var(--mdc-border); border-radius: var(--mdc-radius-md); overflow: hidden; background: var(--mdc-panel); }
  .host { position: absolute; inset: 0; }
  .host.hidden { visibility: hidden; }
  .fallback { position: absolute; inset: 0; margin: 0; padding: .5rem .75rem; overflow: auto; font-family: var(--mdc-mono); font-size: 13px; white-space: pre; }
</style>
