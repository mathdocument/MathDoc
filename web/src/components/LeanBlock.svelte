<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import type { NodeDetail, SrcBlock } from "../lib/types";
  import type { Theme } from "../lib/theme";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import { removeDraft, setDraftDirty, trackMutation } from "../lib/unsaved";
  interface Props {
    fnode: string; revision: string; block?: SrcBlock; theme: Theme; active?: boolean; selection?: number;
    onDeleted?: (node: NodeDetail, srctype: string) => void;
    onSaved?: (node: NodeDetail) => void; onReady?: () => void;
  }
  let { fnode, revision, block, theme, selection = 0, onDeleted, onSaved, onReady }: Props = $props();
  let frame = $state<HTMLIFrameElement>();
  let session = $state<string | null>(null);
  let content = $state("");
  let baseline = $state("");
  let dirty = $derived(content !== baseline);
  let action = $state<"save" | "check" | "build" | "delete" | null>(null);
  let busy = $derived(action !== null);
  let error = $state<string | null>(null);
  let result = $state<Awaited<ReturnType<typeof api.checkLean>> | null>(null);
  let ready = $state(false);
  let runtimeReady = $state(false);
  let opening = false;
  let connection = 0;
  let progress = $state("");
  let generation = 0;
  let openedNode = "";
  let openedSelection = -1;
  let initialTheme = $state<Theme>("light");
  let alive = true;
  const draft = Symbol("Lean draft");
  $effect(() => { setDraftDirty(draft, !!block && dirty); });
  $effect(() => {
    const node = block ? fnode : "";
    if (node === openedNode && selection === openedSelection) return;
    // Navigation has already confirmed discarding edits. Reset the old native
    // document as well, so its retained worker never carries an abandoned draft.
    untrack(() => frame?.contentWindow?.postMessage({ type: "lean-source", fnode: openedNode, value: baseline }, location.origin));
    openedNode = node; openedSelection = selection;
    untrack(() => {
      generation++; ready = false; error = null; result = null; progress = "";
      content = block?.content ?? ""; baseline = content;
      if (node) {
        if (!session && !opening) void open();
        else selectNode();
      }
    });
  });
  $effect(() => { frame?.contentWindow?.postMessage({ type: "lean-theme", value: theme }, location.origin); });
  $effect(() => {
    const next = block?.content;
    if (next === undefined) return;
    if (next === baseline) return;
    if (!dirty) { content = next; frame?.contentWindow?.postMessage({ type: "lean-source", fnode, value: next }, location.origin); }
    baseline = next;
  });
  function selectNode() {
    if (runtimeReady && block) frame?.contentWindow?.postMessage({ type: "lean-select", fnode, revision, generation }, location.origin);
  }
  async function open() {
    const current = ++connection;
    closeSession();
    opening = true; runtimeReady = false; ready = false; error = null; result = null; initialTheme = theme;
    try {
      const response = await api.leanSession(fnode, revision);
      if (alive && connection === current) session = response.id;
      else void api.closeLeanSession(response.id).catch(console.warn);
    } catch (e) { if (alive && connection === current) { error = errMsg(e); onReady?.(); } }
    finally { if (connection === current) opening = false; }
  }
  function closeSession() {
    if (session) void api.closeLeanSession(session).catch(console.warn);
    session = null; runtimeReady = false;
  }
  async function save(check = false, build = false) {
    if (busy) return;
    const current = generation, node = fnode, source = content;
    action = build ? "build" : check ? "check" : "save"; error = null; result = null;
    const release = trackMutation();
    try {
      let rev = revision;
      if (dirty) {
        const updated = await api.putBlock(node, "lean", source, rev);
        if (!alive || generation !== current) return;
        baseline = source; rev = updated.revision; onSaved?.(updated);
      }
      if (check) {
        const checked = await api.checkLean(node, rev, build);
        if (alive && generation === current) { result = checked; onSaved?.((await api.nodeView(node)).node); }
      }
    } catch (e) { if (alive && generation === current) error = errMsg(e); }
    finally { release(); if (alive && generation === current) action = null; }
  }
  async function remove() {
    if (busy || !confirm("Delete the Lean block from this node?")) return;
    action = "delete"; const release = trackMutation();
    try { const updated = await api.deleteBlock(fnode, "lean", revision); onDeleted?.(updated, "lean"); }
    catch (e) { error = errMsg(e); }
    finally { action = null; release(); }
  }
  function message(event: MessageEvent) {
    if (event.origin !== location.origin || event.source !== frame?.contentWindow) return;
    if (event.data?.type === "lean-runtime-ready") { runtimeReady = true; selectNode(); return; }
    if (event.data?.type === "lean-error") { error = String(event.data.value); onReady?.(); return; }
    if (event.data?.fnode !== fnode || !block) return;
    switch (event.data?.type) {
      case "lean-change": if (typeof event.data.value === "string") { content = event.data.value; result = null; } break;
      case "lean-save": void save(); break;
      case "lean-check": void save(true); break;
      case "lean-ready": if (event.data.generation === generation) { ready = true; onReady?.(); } break;
      case "lean-progress": progress = String(event.data.value); break;

    }
  }
  onDestroy(() => { alive = false; generation++; connection++; closeSession(); removeDraft(draft); });
</script>

<svelte:window onmessage={message} />
<article class="lean-block" class:hidden={!block} data-srctype="lean">
  <header>
    <strong>Lean</strong><span class="unsaved">{dirty ? "Unsaved" : ""}</span>
    <div class="actions">
    <button onclick={() => void save()} disabled={busy || !dirty}>Save</button>
    <button onclick={() => void save(true)} disabled={busy} aria-busy={action === "check"}>Save & check</button>
    <button onclick={() => void save(true, true)} disabled={busy}>Save & build</button>
    <button onclick={() => void open()} disabled={dirty || busy}>Reload environment</button>
    <button onclick={() => void remove()} disabled={busy} aria-label="Delete Lean block">Delete</button>
    </div>
  </header>
  {#if action}<div class="status activity" role="status" aria-live="polite">{{ save: "Saving…", check: "Checking…", build: "Building…", delete: "Deleting…" }[action]}</div>{/if}
  {#if !ready && !error}<div class="status" aria-busy="true">{session && runtimeReady ? "Opening Lean node…" : "Starting Lean editor…"}</div>{/if}
  {#if ready && progress}<div class="status" role="status">{progress}</div>{/if}
  {#if session}
    <div class:pending={!ready} inert={!ready}><iframe bind:this={frame} title="Lean source and Infoview" src={`/lean.html?session=${encodeURIComponent(session)}&theme=${initialTheme}`} allow="clipboard-write"></iframe></div>
  {/if}
  {#if error}<div class="error" role="alert">{error}</div>{/if}
  {#if result}
    <div class="status" class:error={!result.certified}>
      {result.certified ? "Checked" : result.passed ? "Dependency check failed" : "Lean errors"}
      {result.built ? "· olean ready" : ""} · {result.cache_hit ? "cached" : `${result.elapsed_ms} ms`}
      {#each result.dependency_errors as issue}<div>{issue}</div>{/each}
    </div>
  {/if}
</article>
<style>
  .lean-block { border: 1px solid var(--mdc-border); border-radius: var(--mdc-radius-md); overflow: hidden; flex-shrink: 0; }
  .hidden { display:none; }
  .pending { visibility:hidden; }
  header { display:flex; flex-wrap:wrap; align-items:center; gap:.5rem; padding:.6rem; background:var(--mdc-card); font-size:.75rem; }
  .unsaved { flex:1; color:var(--mdc-warning); }
  .actions { display:flex; flex-wrap:wrap; gap:.5rem; }
  button { flex:0 0 auto; white-space:nowrap; background:var(--mdc-bg); color:var(--mdc-fg); border:1px solid var(--mdc-border); padding:.3rem .5rem; border-radius:4px; cursor:pointer; }
  button:disabled { opacity:.5; cursor:default; }
  iframe { display:block; width:100%; height:500px; border:0; }
  .status,.error { padding:.6rem; font-size:.8rem; color:var(--mdc-muted); }
  .error { color:var(--mdc-error); }
</style>
