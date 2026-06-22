<script lang="ts">
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";

  interface Props {
    onCreated: (fnode: string) => void;
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

  $effect(() => {
    titleInputEl?.focus();
  });

  $effect(() => {
    if (step === "file") fileInputEl?.focus();
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      if (step === "file") {
        step = "title";
      } else {
        onClose();
      }
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      if (step === "title") {
        if (title.trim().length === 0) {
          error = "title must be non-empty";
          return;
        }
        error = null;
        step = "file";
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
    saving = true;
    error = null;
    try {
      const params: { title: string; file?: string } = { title: title.trim() };
      if (file.trim().length > 0) params.file = file.trim();
      const node = await api.newNode(params);
      onCreated(node.fnode);
      onClose();
    } catch (e) {
      error = errMsg(e);
    } finally {
      saving = false;
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onClose} role="presentation">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="dialog" role="dialog" aria-label="new node" tabindex="-1" onclick={(e) => e.stopPropagation()}>
    <h2>new node</h2>
    <label class="field" class:active={step === "title"}>
      <span class="lbl">title</span>
      <input
        bind:this={titleInputEl}
        bind:value={title}
        placeholder="New Lemma"
        autocomplete="off"
        disabled={step !== "title"}
      />
    </label>
    <label class="field" class:active={step === "file"}>
      <span class="lbl">file</span>
      <input
        bind:this={fileInputEl}
        bind:value={file}
        placeholder="(default: <fnode>.mdoc at workspace root)"
        autocomplete="off"
        spellcheck="false"
        disabled={step !== "file"}
      />
    </label>
    {#if error}
      <div class="error-bar">{error}</div>
    {/if}
    <div class="hint">
      {#if step === "title"}
        Enter: next · Esc: cancel
      {:else}
        Enter: create · Esc: back
      {/if}
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 18vh;
    z-index: 50;
  }
  .dialog {
    width: min(520px, 90vw);
    background: var(--mdc-panel);
    border: 1px solid var(--mdc-border-strong);
    border-radius: 6px;
    overflow: hidden;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.35);
    padding: 0.8rem 1rem;
  }
  h2 {
    margin: 0 0 0.8rem;
    font-size: 0.95rem;
    color: var(--mdc-accent);
    font-family: var(--mdc-mono);
  }
  .field {
    display: block;
    margin-bottom: 0.6rem;
    opacity: 0.45;
  }
  .field.active {
    opacity: 1;
  }
  .lbl {
    display: block;
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--mdc-dim);
    margin-bottom: 0.2rem;
  }
  input {
    width: 100%;
    box-sizing: border-box;
    background: var(--mdc-bg);
    color: var(--mdc-fg);
    border: 1px solid var(--mdc-border);
    border-radius: 3px;
    padding: 0.5rem 0.6rem;
    font-size: 0.95rem;
    font-family: inherit;
  }
  .field.active input {
    border-color: var(--mdc-accent);
  }
  input:focus {
    outline: none;
  }
  input:disabled {
    cursor: default;
  }
  .error-bar {
    margin-top: 0.4rem;
    padding: 0.4rem 0.6rem;
    background: rgba(247, 118, 142, 0.12);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.78rem;
    border-radius: 3px;
  }
  .hint {
    margin-top: 0.6rem;
    font-size: 0.72rem;
    color: var(--mdc-dim);
  }
</style>
