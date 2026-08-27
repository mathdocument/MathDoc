<script lang="ts">
  import { onDestroy } from "svelte";
  import { ArrowLeft, ArrowRight, FilePlus2, X } from "@lucide/svelte";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import { modal } from "../lib/modal";
  import {
    confirmDiscardDraft,
    confirmDiscardDrafts,
    removeDraft,
    setDraftDirty,
    setMutationPending,
  } from "../lib/unsaved";

  interface Props {
    onCreated: (fnode: string, skipUnsavedGuard: boolean) => void;
    onClose: () => void;
  }
  let { onCreated, onClose }: Props = $props();

  let title = $state("");
  let file = $state("");
  let step = $state<"title" | "file">("title");
  let saving = $state(false);
  let error: string | null = $state(null);
  let titleInputEl = $state<HTMLInputElement | null>(null);
  let fileInputEl = $state<HTMLInputElement | null>(null);
  let alive = true;
  const draftId = Symbol("new node draft");
  const mutationId = Symbol("new node mutation");

  onDestroy(() => {
    alive = false;
    removeDraft(draftId);
  });

  $effect(() => {
    setDraftDirty(draftId, title.trim().length > 0 || file.trim().length > 0);
  });

  function close() {
    if (!saving && confirmDiscardDraft(draftId)) onClose();
  }

  $effect(() => {
    titleInputEl?.focus();
  });

  $effect(() => {
    if (step === "file") fileInputEl?.focus();
  });

  function advance() {
    if (title.trim().length === 0) {
      error = "title must be non-empty";
      return;
    }
    error = null;
    step = "file";
  }

  function onKey(e: KeyboardEvent) {
    if ((e.key === "Enter" || e.key === " ") &&
      e.target instanceof Element && e.target.closest(".close-btn, .actions")) return;
    if (e.key === "Escape") {
      if (saving) return;
      e.preventDefault();
      if (step === "file") {
        step = "title";
      } else {
        close();
      }
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      if (step === "title") {
        advance();
      } else {
        void submit();
      }
    }
  }

  async function submit() {
    if (saving) return;
    if (title.trim().length === 0) {
      step = "title";
      error = "title must be non-empty";
      return;
    }
    if (!confirmDiscardDrafts(draftId)) return;
    saving = true;
    let pending = true;
    setMutationPending(mutationId, true);
    error = null;
    try {
      const params: { title: string; file?: string } = { title: title.trim() };
      if (file.trim().length > 0) params.file = file.trim();
      const node = await api.newNode(params);
      setMutationPending(mutationId, false);
      pending = false;
      if (!alive) return;
      removeDraft(draftId);
      onCreated(node.fnode, true);
      onClose();
    } catch (e) {
      if (alive) error = errMsg(e);
    } finally {
      if (pending) setMutationPending(mutationId, false);
      if (alive) saving = false;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={close} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-label="new node"
    tabindex="-1"
    use:modal
    onclick={(e) => e.stopPropagation()}
  >
    <header class="dialog-head">
      <span class="head-icon"><FilePlus2 size={16} strokeWidth={1.8} /></span>
      <span><small>Workspace</small><h2>New node</h2></span>
      <span class="step">Step {step === "title" ? "1" : "2"} of 2</span>
      <button class="close-btn" onclick={close} title="Close" aria-label="Close new node"><X size={17} strokeWidth={1.8} /></button>
    </header>
    <div class="form-body">
      <label class="field" class:active={step === "title"}>
        <span class="lbl">Title</span>
        <input
          bind:this={titleInputEl}
          bind:value={title}
          placeholder="New Lemma"
          autocomplete="off"
          disabled={step !== "title"}
        />
      </label>
      <label class="field" class:active={step === "file"}>
        <span class="lbl">File path <small>Optional</small></span>
        <input
          bind:this={fileInputEl}
          bind:value={file}
          placeholder="Default: <fnode>.mdoc at workspace root"
          autocomplete="off"
          spellcheck="false"
          disabled={step !== "file"}
        />
      </label>
      {#if error}
        <div class="error-bar">{error}</div>
      {/if}
    </div>
    <footer class="dialog-footer">
      <div class="hint"><kbd>Enter</kbd> {step === "title" ? "Next" : "Create"} <span>·</span> <kbd>Esc</kbd> {step === "title" ? "Cancel" : "Back"}</div>
      <div class="actions">
        {#if step === "title"}
          <button class="secondary" onclick={close} disabled={saving}>Cancel</button>
          <button class="primary" onclick={advance} disabled={saving}>Next <ArrowRight size={14} strokeWidth={1.9} /></button>
        {:else}
          <button class="secondary" onclick={() => (step = "title")} disabled={saving}><ArrowLeft size={14} strokeWidth={1.9} />Back</button>
          <button class="primary" onclick={() => void submit()} disabled={saving}>Create node</button>
        {/if}
      </div>
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: var(--mdc-backdrop);
    backdrop-filter: blur(6px);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 14vh;
    z-index: 50;
    animation: mdc-fade-in 150ms ease;
  }
  .dialog {
    width: min(560px, 90vw);
    background: color-mix(in srgb, var(--mdc-panel) 98%, transparent);
    border: 1px solid var(--mdc-border-strong);
    border-radius: var(--mdc-radius-lg);
    overflow: hidden;
    box-shadow: var(--mdc-shadow-panel);
    animation: mdc-pop-in 180ms cubic-bezier(0.2, 0.8, 0.3, 1);
  }
  .dialog-head {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    min-height: 64px;
    padding: 0 0.85rem 0 1rem;
    background: var(--mdc-panel-raised);
    border-bottom: 1px solid var(--mdc-border);
  }
  .head-icon {
    display: grid;
    place-items: center;
    width: 31px;
    height: 31px;
    color: var(--mdc-accent);
    background: rgba(124, 156, 255, 0.1);
    border-radius: 8px;
  }
  .dialog-head > span:nth-child(2) {
    display: flex;
    flex-direction: column;
    gap: 0.12rem;
  }
  .dialog-head small {
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: 0.58rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  h2 {
    margin: 0;
    color: var(--mdc-fg);
    font-size: 0.9rem;
    font-weight: 630;
  }
  .step {
    margin-left: auto;
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: 0.62rem;
  }
  .close-btn {
    display: grid;
    place-items: center;
    width: 31px;
    height: 31px;
    padding: 0;
    color: var(--mdc-muted);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--mdc-radius-sm);
    cursor: pointer;
  }
  .close-btn:hover {
    color: var(--mdc-fg);
    background: var(--mdc-card-hover);
    border-color: var(--mdc-border);
  }
  .form-body {
    padding: 1rem;
  }
  .field {
    display: block;
    margin-bottom: 0.85rem;
    opacity: 0.38;
    transition: opacity 120ms ease;
  }
  .field.active {
    opacity: 1;
  }
  .lbl {
    display: block;
    font-size: 0.65rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--mdc-muted);
    margin-bottom: 0.35rem;
  }
  .lbl small {
    margin-left: 0.3rem;
    color: var(--mdc-muted);
    font-size: 0.55rem;
  }
  input {
    width: 100%;
    box-sizing: border-box;
    background: var(--mdc-code-bg);
    color: var(--mdc-code-fg);
    border: 1px solid var(--mdc-border);
    border-radius: 8px;
    padding: 0.65rem 0.7rem;
    font-size: 0.85rem;
    font-family: inherit;
  }
  .field.active input {
    border-color: var(--mdc-accent);
    box-shadow: 0 0 0 3px rgba(124, 156, 255, 0.08);
  }
  input:disabled {
    cursor: default;
  }
  .error-bar {
    padding: 0.5rem 0.6rem;
    background: rgba(255, 125, 143, 0.1);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.7rem;
    border-radius: var(--mdc-radius-sm);
  }
  .dialog-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    min-height: 60px;
    padding: 0.65rem 0.8rem;
    border-top: 1px solid var(--mdc-border);
    background: color-mix(in srgb, var(--mdc-bg) 48%, transparent);
  }
  .hint {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 0.62rem;
    color: var(--mdc-muted);
  }
  kbd {
    padding: 0.13rem 0.3rem;
    color: var(--mdc-dim);
    background: var(--mdc-card);
    border: 1px solid var(--mdc-border);
    border-radius: 4px;
    font-family: var(--mdc-mono);
    font-size: 0.57rem;
  }
  .actions {
    display: flex;
    gap: 0.45rem;
  }
  .actions button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.35rem;
    min-height: 32px;
    padding: 0 0.72rem;
    border-radius: var(--mdc-radius-sm);
    font-size: 0.68rem;
    font-weight: 600;
    cursor: pointer;
  }
  .actions button:disabled {
    opacity: 0.35;
    cursor: default;
  }
  .secondary {
    color: var(--mdc-fg-soft);
    background: transparent;
    border: 1px solid var(--mdc-border);
  }
  .primary {
    color: var(--mdc-on-accent);
    background: var(--mdc-accent);
    border: 1px solid var(--mdc-accent);
  }
</style>
