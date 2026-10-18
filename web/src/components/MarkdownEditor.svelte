<script lang="ts">
  // A short Markdown text (a note, a review comment) in Monaco, like a text block: the
  // Markdown toolbar, Edit/Preview with the preview rendered from the draft, a character count
  // and the caller's buttons. Monaco loads on demand (lib/markdown-monaco.ts).
  import { onDestroy, onMount, untrack, type Snippet } from "svelte";
  import { Code2, Eye } from "@lucide/svelte";
  import type { MarkdownEditorHandle } from "../lib/markdown-monaco";
  import { errMsg } from "../lib/format";
  import MarkdownPreview from "./MarkdownPreview.svelte";
  import MarkdownToolbar from "./MarkdownToolbar.svelte";
  let { value = $bindable(""), label, placeholder = "", maxlength, focus = false, onSubmit, onCancel, actions }: {
    value?: string; label: string; placeholder?: string; maxlength?: number;
    /** Focus the editor once it is ready, e.g. when an edit starts. */
    focus?: boolean;
    /** ⌘↵ and Esc. */
    onSubmit?: () => void; onCancel?: () => void;
    /** Buttons at the right of the footer, e.g. Save. */
    actions?: Snippet;
  } = $props();
  let host = $state<HTMLDivElement>();
  let handle = $state<MarkdownEditorHandle | null>(null);
  let failed = $state<string | null>(null);
  let previewing = $state(false);
  let alive = true;
  const length = $derived(value.trim().length);

  onMount(async () => {
    try {
      const { createMarkdownEditor } = await import("../lib/markdown-monaco");
      if (!alive || !host) return;
      const h = await createMarkdownEditor(host, {
        value: untrack(() => value), label, placeholder,
        onChange: (v) => (value = v), onSubmit: () => onSubmit?.(), onCancel: () => onCancel?.(),
      });
      if (!alive) return h.dispose();
      handle = h;
      if (focus) h.focus();
    } catch (e) {
      if (alive) failed = errMsg(e);
    }
  });
  onDestroy(() => { alive = false; handle?.dispose(); });
  // A value set from outside, e.g. cleared after the note was added.
  $effect(() => { const v = value; untrack(() => handle?.setValue(v)); });
</script>

<div class="markdown-editor">
  <div class="md-head">
    <MarkdownToolbar disabled={previewing || !handle} onCommand={(c) => handle?.apply(c)} />
    <span class="spacer"></span>
    <!-- Icons only, so that the toolbar fits one line in the inspector. -->
    <button type="button" class="mode" aria-label="Edit" title="Edit" aria-pressed={!previewing} onclick={() => { previewing = false; handle?.focus(); }}><Code2 size={14} strokeWidth={1.8} /></button>
    <button type="button" class="mode" aria-label="Preview" title="Preview" aria-pressed={previewing} onclick={() => (previewing = true)}><Eye size={14} strokeWidth={1.8} /></button>
  </div>
  <!-- The editor stays mounted while previewing, so its undo history survives. -->
  <div class="md-surface" class:hidden={previewing} class:pending={!handle} bind:this={host}></div>
  {#if !handle && !previewing}<div class="md-loading" aria-busy="true">{failed ?? "Loading editor…"}</div>{/if}
  {#if previewing}
    <div class="md-preview" aria-label={`${label} preview`}>
      {#if value.trim()}<MarkdownPreview compact source={value} />{:else}<p class="hint">Nothing to preview.</p>{/if}
    </div>
  {/if}
  <div class="md-foot">
    {#if maxlength}<span class="hint" class:over={length > maxlength}>{length}/{maxlength}</span>{/if}
    {#if onSubmit}<span class="hint">⌘↵</span>{/if}
    <span class="spacer"></span>
    {@render actions?.()}
  </div>
</div>

<style>
  .markdown-editor { display: flex; flex-direction: column; border: 1px solid var(--mdc-border); border-radius: var(--mdc-radius-sm); background: var(--mdc-code-bg); }
  .markdown-editor:focus-within { border-color: var(--mdc-border-strong); }
  .md-head, .md-foot { display: flex; align-items: center; gap: .15rem; padding: .2rem .3rem; background: var(--mdc-panel); }
  .md-head { border-bottom: 1px solid var(--mdc-border); border-radius: var(--mdc-radius-sm) var(--mdc-radius-sm) 0 0; }
  .md-foot { border-top: 1px solid var(--mdc-border); border-radius: 0 0 var(--mdc-radius-sm) var(--mdc-radius-sm); padding: .3rem .45rem; }
  .spacer { flex: 1; }
  .mode { display: grid; place-items: center; width: 26px; height: 24px; padding: 0; border: 1px solid transparent; border-radius: var(--mdc-radius-sm); background: none; color: var(--mdc-dim); font: inherit; font-size: var(--mdc-text-xs); cursor: pointer; }
  .mode:hover { color: var(--mdc-fg); }
  .mode[aria-pressed="true"] { border-color: var(--mdc-border); background: var(--mdc-card); color: var(--mdc-fg); }
  .md-surface { min-height: 96px; overflow: hidden; }
  .md-surface.hidden { display: none; }
  .md-surface.pending { visibility: hidden; height: 0; min-height: 0; }
  .md-loading { display: grid; place-items: center; min-height: 96px; color: var(--mdc-code-dim); font-size: var(--mdc-text-xs); }
  .md-preview { min-height: 96px; padding: .55rem .7rem; overflow-wrap: anywhere; font-size: var(--mdc-text-sm); line-height: 1.6; background: var(--mdc-panel); }
  .hint { color: var(--mdc-muted); font-size: var(--mdc-text-xs); }
  .hint.over { color: var(--mdc-error); }
  .md-preview .hint { margin: 0; }
</style>
