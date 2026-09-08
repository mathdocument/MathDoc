<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import type { NodeDetail, SrcBlock } from "../lib/types";
  import type { Theme } from "../lib/theme";
  import { api } from "../lib/api";
  import { errMsg } from "../lib/format";
  import { removeDraft, setDraftDirty, trackMutation } from "../lib/unsaved";
  interface Props {
    fnode: string; revision: string; block: SrcBlock; theme: Theme; active?: boolean;
    onDeleted?: (node: NodeDetail, srctype: string) => void;
    onSaved?: (node: NodeDetail) => void; onReady?: () => void;
  }
  let { fnode, revision, block, theme, onDeleted, onSaved, onReady }: Props = $props();
  let frame = $state<HTMLIFrameElement>();
  let session = $state<string | null>(null);
  let content = $state("");
  let baseline = $state("");
  let dirty = $derived(content !== baseline);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let result = $state<Awaited<ReturnType<typeof api.checkLean>> | null>(null);
  let ready = $state(false);
  let generation = 0;
  let openedNode = "";
  let initialTheme = $state<Theme>("light");
  let alive = true;
  const draft = Symbol("Lean draft");
  $effect(() => { setDraftDirty(draft, dirty); });
  $effect(() => { const node = fnode; if (node !== openedNode) { openedNode = node; untrack(() => void open(node)); } });
  $effect(() => { frame?.contentWindow?.postMessage({ type: "lean-theme", value: theme }, location.origin); });
  $effect(() => {
    const next = block.content;
    if (next === baseline) return;
    if (!dirty) { content = next; frame?.contentWindow?.postMessage({ type: "lean-source", value: next }, location.origin); }
    baseline = next;
  });
  async function open(node = fnode) {
    const current = ++generation;
    closeSession();
    session = null; ready = false; busy = false; error = null; result = null; initialTheme = theme;
    content = block.content; baseline = block.content;
    try {
      const response = await api.leanSession(node, revision);
      if (alive && generation === current) session = response.id;
      else void api.closeLeanSession(response.id).catch(console.warn);
    } catch (e) { if (alive && generation === current) { error = errMsg(e); onReady?.(); } }
  }
  function closeSession() {
    if (session) void api.closeLeanSession(session).catch(console.warn);
    session = null;
  }
  async function save(check = false, build = false) {
    if (busy) return;
    const current = generation, node = fnode, source = content;
    busy = true; error = null;
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
    finally { release(); if (alive && generation === current) busy = false; }
  }
  async function remove() {
    if (busy || !confirm("Delete the Lean block from this node?")) return;
    busy = true; const release = trackMutation();
    try { const updated = await api.deleteBlock(fnode, "lean", revision); onDeleted?.(updated, "lean"); }
    catch (e) { error = errMsg(e); }
    finally { busy = false; release(); }
  }
  function message(event: MessageEvent) {
    if (event.origin !== location.origin || event.source !== frame?.contentWindow) return;
    switch (event.data?.type) {
      case "lean-change": if (typeof event.data.value === "string") { content = event.data.value; result = null; } break;
      case "lean-save": void save(); break;
      case "lean-check": void save(true); break;
      case "lean-ready": ready = true; onReady?.(); break;
      case "lean-error": error = String(event.data.value); closeSession(); onReady?.(); break;
    }
  }
  onDestroy(() => { alive = false; generation++; closeSession(); removeDraft(draft); });
</script>

<svelte:window onmessage={message} />
<article class="lean-block" data-srctype="lean">
  <header>
    <strong>Lean</strong><span>{dirty ? "Unsaved" : ""}</span>
    <button onclick={() => void save()} disabled={busy || !dirty}>Save</button>
    <button onclick={() => void save(true)} disabled={busy}>{busy ? "Checking…" : "Save & check"}</button>
    <button onclick={() => void save(true, true)} disabled={busy}>Save & build</button>
    <button onclick={() => void open()} disabled={dirty || busy}>Reload environment</button>
    <button onclick={() => void remove()} disabled={busy} aria-label="Delete Lean block">Delete</button>
  </header>
  {#if !ready && !error}<div class="status" aria-busy="true">Starting Lean editor…</div>{/if}
  {#if session}
    <iframe bind:this={frame} title="Lean source and Infoview" src={`/lean.html?session=${encodeURIComponent(session)}&theme=${initialTheme}`} allow="clipboard-write"></iframe>
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
  header { display:flex; flex-wrap:wrap; align-items:center; gap:.5rem; padding:.6rem; background:var(--mdc-card); font-size:.75rem; }
  header span { flex:1; color:var(--mdc-warning); }
  button { background:var(--mdc-bg); color:var(--mdc-fg); border:1px solid var(--mdc-border); padding:.3rem .5rem; border-radius:4px; cursor:pointer; }
  button:disabled { opacity:.5; cursor:default; }
  iframe { display:block; width:100%; height:500px; border:0; }
  .status,.error { padding:.6rem; font-size:.8rem; color:var(--mdc-muted); }
  .error { color:var(--mdc-error); }
</style>
