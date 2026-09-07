<script lang="ts">
  import { onDestroy } from "svelte";
  import { FilePlus2, X } from "@lucide/svelte";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import { modal } from "../lib/modal";
  import {
    confirmDiscardDraft,
    confirmDiscardDrafts,
    removeDraft,
    setDraftDirty,
    trackMutation,
  } from "../lib/unsaved";

  interface Props {
    disabled: boolean;
    onCreated: (fnode: string, skipUnsavedGuard: boolean) => void;
    onClose: () => void;
  }
  let { disabled, onCreated, onClose }: Props = $props();

  let title = $state("");
  let saving = $state(false);
  let error: string | null = $state(null);
  let titleInputEl = $state<HTMLInputElement | null>(null);
  let alive = true;
  const draftId = Symbol("new node draft");

  onDestroy(() => {
    alive = false;
    removeDraft(draftId);
  });

  $effect(() => {
    setDraftDirty(draftId, title.trim().length > 0);
  });

  function close() {
    if (!saving && confirmDiscardDraft(draftId)) onClose();
  }

  $effect(() => {
    titleInputEl?.focus();
  });

  function onKey(e: KeyboardEvent) {
    if (disabled || saving || e.isComposing) return;
    if ((e.key === "Enter" || e.key === " ") &&
      e.target instanceof Element && e.target.closest(".close-btn, .actions")) return;
    if (e.key === "Enter") {
      e.preventDefault();
      void submit();
    }
  }

  function onCancel(event: Event) {
    event.preventDefault();
    if (disabled || saving) return;
    close();
  }

  async function submit() {
    if (saving) return;
    if (title.trim().length === 0) {
      error = "title must be non-empty";
      return;
    }
    if (!confirmDiscardDrafts(draftId)) return;
    saving = true;
    const clearMutation = trackMutation();
    error = null;
    try {
      const node = await api.newNode({ title: title.trim() });
      clearMutation();
      if (!alive) return;
      removeDraft(draftId);
      onCreated(node.fnode, true);
      onClose();
    } catch (e) {
      if (alive) error = errMsg(e);
    } finally {
      clearMutation();
      if (alive) saving = false;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<dialog
    class="dialog modal-dialog"
    aria-label="new node"
    use:modal
    oncancel={onCancel}
    onclick={(event) => { if (event.target === event.currentTarget) close(); }}
  >
    <header class="dialog-head">
      <span class="head-icon"><FilePlus2 size={16} strokeWidth={1.8} /></span>
      <span><small>Workspace</small><h2>New node</h2></span>
      <button class="close-btn" onclick={close} title="Close" aria-label="Close new node"><X size={17} strokeWidth={1.8} /></button>
    </header>
    <div class="form-body">
      <label class="field">
        <span class="lbl">Title</span>
        <input
          bind:this={titleInputEl}
          bind:value={title}
          placeholder="New Lemma"
          autocomplete="off"
          disabled={saving}
        />
      </label>
      {#if error}
        <div class="error-bar modal-error">{error}</div>
      {/if}
    </div>
    <footer class="dialog-footer">
      <div class="hint"><kbd>Enter</kbd> Create <span>·</span> <kbd>Esc</kbd> Cancel</div>
      <div class="actions">
        <button class="secondary" onclick={close} disabled={saving}>Cancel</button>
        <button class="primary" onclick={() => void submit()} disabled={saving}>Create node</button>
      </div>
    </footer>
  </dialog>

<style>
  .dialog {
    width: min(560px, 90vw);
    margin-top: 14vh;
  }
  .head-icon {
    color: var(--mdc-accent);
    background: color-mix(in srgb, var(--mdc-accent) 12%, transparent);
  }
  .form-body {
    padding: 1rem 1rem 0.5rem;
  }
  .field {
    display: block;
    margin-bottom: 1rem;
    transition: opacity 180ms var(--mdc-ease);
  }
  .lbl {
    display: block;
    font-size: var(--mdc-text-2xs);
    font-weight: 620;
    text-transform: uppercase;
    letter-spacing: var(--mdc-tracking-label);
    color: var(--mdc-dim);
    margin-bottom: 0.4rem;
  }
  input {
    width: 100%;
    box-sizing: border-box;
    background: var(--mdc-bg);
    color: var(--mdc-fg);
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-sm);
    padding: 0.6rem 0.7rem;
    font-size: var(--mdc-text-md);
    font-family: inherit;
    transition: border-color var(--mdc-dur-fast) var(--mdc-ease),
      box-shadow var(--mdc-dur-fast) var(--mdc-ease);
  }
  input:focus-visible {
    outline: none;
  }
  input:focus-visible {
    border-color: var(--mdc-accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--mdc-ring) 28%, transparent);
  }
  input:disabled {
    cursor: default;
  }
  .error-bar {
    padding: 0.55rem 0.7rem;
    border-radius: var(--mdc-radius-sm);
  }
  .dialog-footer {
    min-height: 58px;
  }
  .hint {
    gap: 0.35rem;
    font-size: var(--mdc-text-2xs);
  }
  kbd {
    padding: 0.12rem 0.3rem;
    font-size: 0.6rem;
  }
  .actions button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.35rem;
  }
  .primary {
    color: var(--mdc-on-accent);
    background: var(--mdc-accent);
    border: 1px solid var(--mdc-accent);
  }
  .primary:hover:not(:disabled) {
    background: var(--mdc-accent-strong);
    border-color: var(--mdc-accent-strong);
  }
</style>
