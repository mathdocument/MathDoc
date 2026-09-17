<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import { ChevronDown, ChevronRight, Save, Trash2, RotateCcw, Play, Square } from "@lucide/svelte";
  import type { NodeDetail, SrcBlock } from "../lib/types";
  import type { Theme } from "../lib/theme";
  import { api } from "../lib/api";
  import { projectPath } from "../lib/project-path";
  import { errMsg } from "../lib/format";
  import { removeDraft, setDraftDirty, trackMutation } from "../lib/unsaved";
  interface Props {
    fnode: string; revision: string; module?: string; block?: SrcBlock; theme: Theme; active?: boolean; selection?: number;
    onDeleted?: (node: NodeDetail, srctype: string) => void;
    onSaved?: (node: NodeDetail) => void; onReady?: () => void;
  }
  let { fnode, revision, module, block, theme, selection = 0, onDeleted, onSaved, onReady }: Props = $props();
  let frame = $state<HTMLIFrameElement>();
  let session = $state<string | null>(null);
  let mounted = $state(false);
  let content = $state("");
  let baseline = $state("");
  let dirty = $derived(content !== baseline);
  let action = $state<"save" | "delete" | null>(null);
  let validating = $state(false);
  let busy = $derived(action !== null);
  let error = $state<string | null>(null);
  let result = $state<Awaited<ReturnType<typeof api.checkLean>> | null>(null);
  let ready = $state(false);
  let runtimeReady = $state(false);
  let opening = $state(false);
  let stopping = $state(false);
  let reconnects = 0;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let connection = 0;
  const closing = new Map<string, () => void>();
  let progress = $state("");
  let generation = 0;
  let openedNode = "";
  let openedSelection = -1;
  let initialTheme = $state<Theme>("light");
  let alive = true;
  let expanded = $state(true);
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
      generation++; ready = false; error = null; result = null; progress = ""; validating = false;
      content = block?.content ?? ""; baseline = content;
      if (node) {
        if (!mounted && !opening) { initialTheme = theme; mounted = true; }
        selectNode();
      } else frame?.contentWindow?.postMessage({ type: "lean-cancel" }, location.origin);
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
  $effect(() => {
    if (runtimeReady && block) frame?.contentWindow?.postMessage({ type: "lean-saved", fnode, revision, source: block.content }, location.origin);
  });
  function selectNode() {
    if (runtimeReady && block) frame?.contentWindow?.postMessage({ type: "lean-select", fnode, revision, generation, module, source: content }, location.origin);
  }
  async function open() {
    if (!block || !runtimeReady) return;
    clearTimeout(reconnectTimer); reconnectTimer = undefined;
    const current = ++connection;
    const previous = closeSession();
    opening = true; error = null; result = null;
    frame?.contentWindow?.postMessage({ type: "lean-starting" }, location.origin);
    try {
      await previous;
      // Allocation prepares one module. If navigation overtakes it, retire that
      // session before connecting a model that the backend has not prepared.
      while (alive && connection === current && block) {
        const node = fnode;
        const response = await api.leanSession(node, revision);
        if (!alive || connection !== current) { void api.closeLeanSession(response.id).catch(console.warn); return; }
        if (fnode !== node) { await api.closeLeanSession(response.id); continue; }
        session = response.id;
        frame?.contentWindow?.postMessage({ type: "lean-start", id: session, fnode, revision, generation, module, source: content }, location.origin);
        break;
      }
    } catch (e) {
      if (alive && connection === current) {
        error = errMsg(e);
        frame?.contentWindow?.postMessage({ type: "lean-stop" }, location.origin);
      }
    } finally { if (connection === current) opening = false; }
  }
  function refresh() {
    if (!session) { reconnects = 0; void open(); return; }
    error = null; result = null;
    frame?.contentWindow?.postMessage({ type: "lean-recheck", fnode, revision, generation, module, source: content }, location.origin);
  }
  async function stop() {
    const current = ++connection;
    clearTimeout(reconnectTimer); reconnectTimer = undefined;
    stopping = true; opening = false; validating = false;
    error = null; result = null; progress = "";
    try { await closeSession(); }
    catch (e) { if (alive && connection === current) error = errMsg(e); }
    finally { if (alive && connection === current) stopping = false; }
  }
  function disconnected(reason: string) {
    error = reason; progress = "";
    // One automatic attempt per editor lifetime; repeated crashes need an explicit
    // retry. The editor and its unsaved model survive the connection change.
    if (!reconnectTimer && reconnects < 1 && block) {
      reconnects++;
      progress = "Reconnecting Lean…";
      reconnectTimer = setTimeout(() => { if (alive) void open(); }, 500);
    }
  }
  function closeSession() {
    const id = session;
    session = null;
    const stopped = id && alive && runtimeReady ? new Promise<void>(resolve => {
      const done = () => { clearTimeout(timer); closing.delete(id); resolve(); };
      // A wedged initialization cannot acknowledge shutdown; the backend still
      // reaps its process after this bounded grace period.
      const timer = setTimeout(done, 1500);
      closing.set(id, done);
    }) : Promise.resolve();
    frame?.contentWindow?.postMessage({ type: "lean-stop", session: id }, location.origin);
    return stopped.then(() => id ? api.closeLeanSession(id) : undefined);
  }
  async function save() {
    if (busy) return;
    const current = generation, node = fnode, source = content;
    action = "save"; error = null; result = null;
    const release = trackMutation();
    try {
      let rev = revision;
      if (dirty) {
        const updated = await api.putBlock(node, "lean", source, rev);
        if (!alive || generation !== current) return;
        baseline = source; rev = updated.revision; onSaved?.(updated);
      }
      frame?.contentWindow?.postMessage({ type: "lean-saved", fnode: node, revision: rev, source }, location.origin);
    } catch (e) { if (alive && generation === current) error = errMsg(e); }
    finally { release(); if (alive && generation === current) action = null; }
  }
  async function certified(checked: Awaited<ReturnType<typeof api.checkLean>>) {
    const current = generation, node = fnode;
    if (dirty || checked.revision !== revision) return;
    result = checked; error = null;
    try {
      const view = await api.nodeView(node);
      if (alive && generation === current && revision === checked.revision && !dirty) onSaved?.(view.node);
    } catch (e) { if (alive && generation === current) error = errMsg(e); }
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
    if (event.data?.type === "lean-stopped") { closing.get(event.data.session)?.(); return; }
    if (event.data?.session && event.data.session !== session) return;
    if (event.data?.type === "lean-runtime-ready") { runtimeReady = true; selectNode(); return; }
    if (event.data?.type === "lean-disconnected") { if (session && !stopping) disconnected(String(event.data.value)); return; }
    if (event.data?.type === "lean-error") { error = String(event.data.value); onReady?.(); return; }
    if (event.data?.fnode !== fnode || !block) return;
    switch (event.data?.type) {
      case "lean-change": if (typeof event.data.value === "string") { content = event.data.value; result = null; } break;
      case "lean-save": void save(); break;
      case "lean-certified": void certified(event.data.value); break;
      case "lean-validating": validating = !!event.data.value; break;
      case "lean-validation-error": error = String(event.data.value); break;
      case "lean-ready": if (event.data.generation === generation) { ready = true; onReady?.(); } break;
      case "lean-progress": progress = String(event.data.value); break;

    }
  }
  onDestroy(() => { clearTimeout(reconnectTimer); alive = false; generation++; connection++; void closeSession().catch(console.warn); removeDraft(draft); });
</script>

<svelte:window onmessage={message} />
<article class="source-block lean-block" class:hidden={!block} class:preparing={!ready && !error && !opening && !session} data-srctype="lean" data-session={session}>
  <header class="block-head">
    <span class="srctype">lean</span><span class="spacer"></span>
    {#if dirty}<span class="dirty" title="Unsaved changes"><span class="dirty-dot"></span><span class="btn-label">Unsaved</span></span>{/if}
    <div class="block-actions">
      <button class="icon-btn expand" onclick={refresh} disabled={opening || stopping || busy || !runtimeReady} aria-label={session ? "Recheck Lean" : "Start Lean server"} title={session ? "Recheck current Lean file" : "Start Lean server"}>{#if session}<RotateCcw size={14} strokeWidth={1.8}/>{:else}<Play size={14} strokeWidth={1.8}/>{/if}</button>
      <button class="icon-btn expand" onclick={() => void stop()} disabled={stopping || (!session && !opening)} aria-label="Stop Lean server" title="Stop this page’s Lean server"><Square size={14} strokeWidth={1.8}/></button>
      <button class="icon-btn expand" onclick={() => expanded = !expanded} aria-expanded={expanded} aria-label={expanded ? "Collapse block" : "Expand block"} title={expanded ? "Collapse" : "Expand"}>{#if expanded}<ChevronDown size={15}/>{:else}<ChevronRight size={15}/>{/if}</button>
      <button class="save" onclick={() => void save()} disabled={busy || !dirty} aria-busy={action === "save"} aria-label="Save" title="Save (Ctrl/⌘+S or Ctrl/⌘+Enter); start Lean server for automatic validation"><Save size={13} strokeWidth={1.9}/><span class="btn-label">Save</span></button>
      <button class="delete" onclick={() => void remove()} disabled={busy} aria-label="Delete block" title="Delete block"><Trash2 size={14} strokeWidth={1.8}/></button>
    </div>
  </header>
  {#if module}
    <details class="module-import"><summary>Lean import</summary><code>import {module}</code></details>
  {/if}
  {#if action}<div class="status activity" role="status" aria-live="polite">{{ save: "Saving…", delete: "Deleting…" }[action]}</div>{/if}
  {#if validating && !dirty}<div class="status activity" role="status" aria-live="polite">Verifying saved version…</div>{/if}
  {#if ready && progress}<div class="status" role="status">{progress}</div>{/if}
  <div class="editor-surface" class:collapsed={!expanded}>
  {#if mounted}
    <div class="native-editor" class:pending={!ready} inert={!ready || !expanded}><iframe bind:this={frame} title="Lean source and Infoview" src={projectPath(`/lean.html?theme=${initialTheme}`)} allow="clipboard-write"></iframe></div>
  {/if}
  </div>
  {#if error}<div class="error-bar" role="alert">{error}</div>{/if}
  {#if result}
    <div class="status" class:error-bar={!result.certified}>
      {result.certified ? "Checked" : result.passed ? "Dependency check failed" : "Lean errors"}
      {result.has_sorry ? "· contains sorry" : ""}
      {result.built ? "· olean ready" : ""} · {result.cache_hit ? "cached" : `${result.elapsed_ms} ms`}
      {#each result.dependency_errors as issue}<div>{issue}</div>{/each}
    </div>
  {/if}
</article>
<style>
  .hidden { display: none; }
  .preparing { visibility:hidden; }
  .module-import { flex-shrink:0; padding: .5rem .65rem; color: var(--mdc-muted); font-size: var(--mdc-text-xs); border-bottom: 1px solid var(--mdc-border); }
  .module-import summary { cursor: pointer; }
  .module-import code { display: block; margin-top: .4rem; overflow-wrap: anywhere; user-select: all; }
  .editor-surface { position:relative; height:100cqh; min-height:0; }
  /* Keep the iframe viewport valid when reloading a collapsed editor. */
  .editor-surface.collapsed { height:0; overflow:hidden; visibility:hidden; }
  .native-editor { height:100%; }
  .collapsed .native-editor { height:100cqh; }
  .pending { visibility: hidden; position: absolute; inset: 0; }
  iframe { display:block; width:100%; height:100%; border:0; }
</style>
