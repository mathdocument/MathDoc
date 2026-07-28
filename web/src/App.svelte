<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    navigate,
    appState,
    goBack,
    goForward,
    canGoBack,
    canGoForward,
    refreshFocused,
    browserHistoryEntry,
    commitFocusedHistory,
    initialHistoryOptions,
    type BrowserHistoryMode,
    type LoadState,
  } from "./lib/state.svelte";
  import { api } from "./lib/api";
  import NodeColumn from "./components/NodeColumn.svelte";
  import EditorPane from "./components/EditorPane.svelte";
  import SearchOverlay from "./components/SearchOverlay.svelte";
  import AddDepOverlay from "./components/AddDepOverlay.svelte";
  import RmDepOverlay from "./components/RmDepOverlay.svelte";
  import NewNodeOverlay from "./components/NewNodeOverlay.svelte";
  import ForceGraph from "./components/ForceGraph.svelte";
  import type { NodeDetail } from "./lib/types";
  import {
    confirmDiscardDrafts,
    hasUnsavedDrafts,
    settlePendingMutations,
    unsavedDraftRevision,
  } from "./lib/unsaved";

  type Overlay =
    | { kind: "none" }
    | { kind: "search" }
    | { kind: "add-dep"; target: string }
    | { kind: "rm-dep"; target: string }
    | { kind: "new-node"; target: string };

  let overlay = $state<Overlay>({ kind: "none" });
  let initialLoad = $state(true);
  let initialError = $state<string | null>(null);
  let refreshError = $state<string | null>(null);

  // Top-level view state: three-column layout vs. full-screen force graph.
  let view = $state<"columns" | "force">("columns");
  // Selected fnode in the force graph (drives the side editor panel).
  let forceSelectedFnode = $state<string | null>(null);
  // Increment after dep mutations to trigger ForceGraph data refresh.
  let graphRevision = $state(0);
  let forceLoadRequest = 0;
  let viewRequest = 0;
  let forceEditorRevision = $state(0);
  // NodeDetail for the force-graph side panel (fetched on selection).
  let forceNodeLoad = $state<LoadState>({ kind: "idle" });

  onMount(() => {
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      if (!hasUnsavedDrafts()) return;
      event.preventDefault();
      event.returnValue = "";
    };
    let restoringHistory = false;
    let popstateRequest = 0;
    const onPopState = async (event: PopStateEvent) => {
      const entry = browserHistoryEntry(event.state);
      if (!entry) return;
      if (restoringHistory) {
        restoringHistory = false;
        return;
      }

      const request = ++popstateRequest;
      const previousIndex = appState.historyIdx;
      const committed = view === "force"
        ? await onForceSelect(entry.fnode, {
            pushHistory: false,
            historyIndex: entry.index,
            browserHistory: "replace",
            preserveOnFailure: true,
          })
        : await navigate(entry.fnode, {
            pushHistory: false,
            historyIndex: entry.index,
            browserHistory: "replace",
          });
      if (request !== popstateRequest || committed) return;
      const delta = previousIndex - entry.index;
      if (delta !== 0) {
        restoringHistory = true;
        window.history.go(delta);
      }
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    window.addEventListener("popstate", onPopState);
    return () => {
      window.removeEventListener("beforeunload", onBeforeUnload);
      window.removeEventListener("popstate", onPopState);
    };
  });

  // Pick a default starting node on first mount: deepest root, else first.
  $effect(() => {
    if (!initialLoad) return;
    initialLoad = false;
    (async () => {
      try {
        const roots = await api.roots();
        if (roots.length === 0) {
          initialError = "workspace has no nodes — run `mdc new -t \"…\"` first";
          return;
        }
        // URL hash can override: #ref=...
        const hash = window.location.hash.slice(1);
        const params = new URLSearchParams(hash);
        const ref = params.get("ref");
        if (ref) {
          const resolved = await api.resolve(ref);
          await navigate(resolved.fnode, initialHistoryOptions(resolved.fnode));
          return;
        }
        const deepest = [...roots].sort((a, b) => b.topo_depth - a.topo_depth)[0]!;
        await navigate(deepest.fnode, initialHistoryOptions(deepest.fnode));
      } catch (e) {
        initialError = e instanceof Error ? e.message : String(e);
      }
    })();
  });

  async function refreshCurrent(skipUnsavedGuard = false): Promise<boolean> {
    if (appState.load.kind !== "ready") return true;
    const fnode = appState.load.node.fnode;
    return navigate(fnode, {
      pushHistory: false,
      skipTransition: true,
      skipUnsavedGuard,
    });
  }

  async function refreshView() {
    if (!confirmDiscardDrafts()) return;
    if (!await settlePendingMutations()) return;
    // Refresh both views so switching between them doesn't show stale data.
    refreshError = null;
    graphRevision++;
    const [forceOk, columnOk] = await Promise.all([
      refreshForceNodeRaw(true),
      refreshCurrent(true),
    ]);
    if (!forceOk || !columnOk) {
      refreshError = "one or more refresh requests failed";
    }
  }

  function refreshForceNode(node: NodeDetail) {
    if (node.fnode !== forceSelectedFnode) return;
    forceLoadRequest++;
    graphRevision++;
    forceNodeLoad = { kind: "ready", node };
  }

  function refreshColumnNode(node: NodeDetail) {
    graphRevision++;
    refreshFocused(node);
  }

  async function refreshForceNodeRaw(skipUnsavedGuard = false): Promise<boolean> {
    if (!forceSelectedFnode) return true;
    if (!skipUnsavedGuard && !confirmDiscardDrafts()) return false;
    if (!await settlePendingMutations()) return false;
    if (!forceSelectedFnode) return true;
    const confirmedDraftRevision = unsavedDraftRevision();
    const targetFnode = forceSelectedFnode;
    const request = ++forceLoadRequest;
    try {
      const node = await api.node(targetFnode);
      if (request !== forceLoadRequest || forceSelectedFnode !== targetFnode) return false;
      if (unsavedDraftRevision() !== confirmedDraftRevision) return false;
      forceEditorRevision++;
      forceNodeLoad = { kind: "ready", node };
      return true;
    } catch (e) {
      if (request === forceLoadRequest && forceSelectedFnode === targetFnode) {
        forceNodeLoad = { kind: "error", message: e instanceof Error ? e.message : String(e) };
      }
      return false;
    }
  }

  function afterDepMutation() {
    graphRevision++;
    if (view === "force") void refreshForceNodeRaw();
    else void refreshCurrent();
  }

  function afterNodeCreated(fnode: string) {
    graphRevision++;
    if (view === "force") void onForceSelect(fnode);
    else void navigate(fnode);
  }

  // The fnode that toolbar actions operate on, regardless of view.
  let activeFnode = $derived(
    view === "force"
      ? forceSelectedFnode
      : appState.load.kind === "ready" ? appState.load.node.fnode : null,
  );
  // Whether the active node is editable (non-broken).
  let activeReady = $derived(
    activeFnode !== null &&
    (view === "force"
      ? forceNodeLoad.kind === "ready" && !forceNodeLoad.node.broken
      : appState.load.kind === "ready" && !appState.load.node.broken),
  );
  // Depens of the active node.
  let activeDepens = $derived(
    view === "force"
      ? (forceNodeLoad.kind === "ready" ? forceNodeLoad.node.depens : [])
      : (appState.load.kind === "ready" ? appState.load.node.depens : []),
  );

  $effect(() => {
    if ("target" in overlay && activeFnode !== overlay.target) {
      overlay = { kind: "none" };
    }
  });

  function statusLine(): string {
    if (view === "force") {
      const s = forceNodeLoad;
      if (s.kind === "ready") return `${s.node.title}  ·  ${s.node.fnode.slice(0, 8)}`;
      if (s.kind === "loading") return "loading…";
      if (s.kind === "error") return `error: ${s.message}`;
      return "";
    }
    const s = appState.load;
    if (s.kind === "ready") {
      return `${s.node.title}  ·  ${s.node.fnode.slice(0, 8)}`;
    }
    if (s.kind === "loading") return "loading…";
    if (s.kind === "error") return `error: ${s.message}`;
    return "";
  }

  async function onForceSelect(
    fnode: string | null,
    opts: {
      skipUnsavedGuard?: boolean;
      pushHistory?: boolean;
      historyIndex?: number;
      browserHistory?: BrowserHistoryMode;
      preserveOnFailure?: boolean;
    } = {},
  ): Promise<boolean> {
    if (!opts.skipUnsavedGuard && !confirmDiscardDrafts()) return false;
    if (!await settlePendingMutations()) return false;
    const previousFnode = forceSelectedFnode;
    const previousLoad = forceNodeLoad;
    const request = ++forceLoadRequest;
    forceSelectedFnode = fnode;
    if (!fnode) {
      forceNodeLoad = { kind: "idle" };
      return true;
    }
    forceNodeLoad = { kind: "loading" };
    await tick();
    const confirmedDraftRevision = unsavedDraftRevision();
    try {
      const node = await api.node(fnode);
      if (request !== forceLoadRequest || forceSelectedFnode !== fnode) return false;
      if (unsavedDraftRevision() !== confirmedDraftRevision) {
        if (opts.preserveOnFailure) {
          forceSelectedFnode = previousFnode;
          forceNodeLoad = previousLoad;
        }
        return false;
      }
      forceEditorRevision++;
      forceNodeLoad = { kind: "ready", node };
      commitFocusedHistory(fnode, {
        pushHistory: opts.pushHistory,
        historyIndex: opts.historyIndex,
        browserHistory: opts.browserHistory,
      });
      return true;
    } catch (e) {
      if (request !== forceLoadRequest || forceSelectedFnode !== fnode) return false;
      if (opts.preserveOnFailure) {
        forceSelectedFnode = previousFnode;
        forceNodeLoad = previousLoad;
      } else {
        forceNodeLoad = {
          kind: "error",
          message: e instanceof Error ? e.message : String(e),
        };
      }
      return false;
    }
  }

  async function toggleGraphView() {
    if (!confirmDiscardDrafts()) return;
    const request = ++viewRequest;
    if (view === "columns") {
      // Enter graph view: select the current column-view node.
      const currentFnode = appState.load.kind === "ready" ? appState.load.node.fnode : null;
      view = "force";
      await tick();
      if (request !== viewRequest) return;
      if (currentFnode) {
        await onForceSelect(currentFnode, {
          skipUnsavedGuard: true,
          pushHistory: false,
        });
      } else {
        forceSelectedFnode = null;
        forceLoadRequest++;
        forceNodeLoad = { kind: "idle" };
      }
    } else {
      // Exit graph view. Keep the force view visible while navigating
      // to avoid flashing the old column-view node (A) before the new
      // one (B) arrives.
      const target = forceSelectedFnode;
      forceSelectedFnode = null;
      forceLoadRequest++;
      forceNodeLoad = { kind: "idle" };
      await tick();
      if (target) {
        await navigate(target, {
          pushHistory: false,
          skipTransition: true,
          skipUnsavedGuard: true,
        });
      }
      if (request !== viewRequest) return;
      view = "columns";
    }
  }
</script>

<div class="app" inert={overlay.kind !== "none"}>
  <header class="toolbar">
    <span class="brand">mdc</span>
    <button
      class="tool"
      onclick={() => void goBack()}
      disabled={!canGoBack()}
      title="back"
    >‹</button>
    <button
      class="tool"
      onclick={() => void goForward()}
      disabled={!canGoForward()}
      title="forward"
    >›</button>
    <button class="tool primary" onclick={() => (overlay = { kind: "search" })} title="search">
      search
    </button>
    <button
      class="tool"
      onclick={() => { if (activeFnode) overlay = { kind: "add-dep", target: activeFnode }; }}
      disabled={!activeReady}
      title="add dependency"
    >+ dep</button>
    <button
      class="tool"
      onclick={() => { if (activeFnode) overlay = { kind: "rm-dep", target: activeFnode }; }}
      disabled={!activeReady || activeDepens.length === 0}
      title="remove dependency"
    >− dep</button>
    <button
      class="tool"
      onclick={() => { if (activeFnode) overlay = { kind: "new-node", target: activeFnode }; }}
      disabled={!activeReady}
      title="create node"
    >+node</button>
    <button
      class="tool"
      onclick={toggleGraphView}
      class:primary={view === "force"}
      title={view === "force" ? "back to columns" : "force-directed graph view"}
    >graph</button>
    <button
      class="tool"
      onclick={refreshView}
      title="refresh — pick up external file changes"
    >refresh</button>
    <span class="spacer"></span>
    <span class="status">{statusLine()}</span>
  </header>
  {#if appState.navigationError || refreshError}
    <div class="app-error" role="alert">
      <span>{appState.navigationError ?? refreshError}</span>
      <button onclick={() => {
        if (appState.failedNavigationFnode) {
          const target = appState.failedNavigationFnode;
          void navigate(target, { pushHistory: appState.load.kind !== "ready" });
        } else {
          void refreshView();
        }
      }}>retry</button>
    </div>
  {/if}

  <!-- Force graph view: always mounted, hidden via CSS when in columns mode. -->
  <main class="force-layout" class:hidden={view !== "force"}>
    <div class="force-canvas-wrap" class:full={!forceSelectedFnode}>
        <ForceGraph
          active={view === "force"}
          onSelect={onForceSelect}
          selectedFnode={forceSelectedFnode}
          revision={graphRevision}
        />
    </div>
    {#if view === "force" && forceSelectedFnode}
      <div class="force-editor-wrap">
        {#key `${forceSelectedFnode}:${forceEditorRevision}`}
          <EditorPane load={forceNodeLoad} onRefresh={refreshForceNode} />
        {/key}
      </div>
    {/if}
  </main>

  <!-- Unmount editors after a confirmed view switch so hidden drafts cannot linger. -->
  {#if view === "columns"}
  <main class="layout">
    {#if initialError}
      <div class="full-error">{initialError}</div>
    {:else}
      <NodeColumn
        title="upstream · referrers"
        items={appState.referrers.items}
        selected={appState.referrers.selected}
        accent="up"
        lastVisitedFnode={appState.lastVisitedFnode}
        onSelect={(fnode) => navigate(fnode, { direction: "up" })}
        onHover={(i) => (appState.referrers.selected = i)}
      />
      {#key appState.editorRevision}
        <EditorPane load={appState.load} onRefresh={refreshColumnNode} />
      {/key}
      <NodeColumn
        title="downstream · dependencies"
        items={appState.children.items}
        selected={appState.children.selected}
        accent="down"
        lastVisitedFnode={appState.lastVisitedFnode}
        onSelect={(fnode) => navigate(fnode, { direction: "down" })}
        onHover={(i) => (appState.children.selected = i)}
      />
    {/if}
  </main>
  {/if}
</div>

{#if overlay.kind === "search"}
  <SearchOverlay
    onPick={(fnode) => {
      overlay = { kind: "none" };
      if (view === "force") {
        void onForceSelect(fnode);
      } else {
        void navigate(fnode, { direction: "neutral" });
      }
    }}
    onClose={() => (overlay = { kind: "none" })}
  />
{/if}

{#if overlay.kind === "add-dep"}
  {#key overlay.target}
    <AddDepOverlay
      targetFnode={overlay.target}
      onAdded={() => afterDepMutation()}
      onClose={() => (overlay = { kind: "none" })}
    />
  {/key}
{/if}

{#if overlay.kind === "rm-dep"}
  {#key overlay.target}
    <RmDepOverlay
      targetFnode={overlay.target}
      onRemoved={() => afterDepMutation()}
      onClose={() => (overlay = { kind: "none" })}
    />
  {/key}
{/if}

{#if overlay.kind === "new-node" && activeReady}
  <NewNodeOverlay
    onCreated={afterNodeCreated}
    onClose={() => (overlay = { kind: "none" })}
  />
{/if}

<style>
  :global(:root) {
    --mdc-bg: #1a1b26;
    --mdc-panel: #1f2335;
    --mdc-card: #24283b;
    --mdc-card-hover: #2d3149;
    --mdc-card-selected: #363b54;
    --mdc-border: #3b3f54;
    --mdc-border-strong: #565f89;
    --mdc-fg: #c0caf5;
    --mdc-dim: #565f89;
    --mdc-accent: #7aa2f7;
    --mdc-accent-up: #bb9af7;
    --mdc-accent-down: #9ece6a;
    --mdc-error: #f7768e;
    --mdc-code-bg: #16161e;
    --mdc-code-fg: #c0caf5;
    --mdc-mono: "SF Mono", "JetBrains Mono", Menlo, Consolas, monospace;
    color-scheme: dark;
  }
  :global(*) {
    box-sizing: border-box;
  }
  :global(html, body) {
    margin: 0;
    height: 100%;
  }
  :global(body) {
    background: var(--mdc-bg);
    color: var(--mdc-fg);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  }
  :global(#app) {
    height: 100vh;
  }
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid var(--mdc-border);
    background: var(--mdc-panel);
    flex-shrink: 0;
  }
  .brand {
    font-weight: 700;
    font-family: var(--mdc-mono);
    color: var(--mdc-accent);
    padding-right: 0.5rem;
  }
  .tool {
    background: var(--mdc-card);
    color: var(--mdc-fg);
    border: 1px solid var(--mdc-border);
    border-radius: 4px;
    padding: 0.3rem 0.6rem;
    font-size: 0.85rem;
    cursor: pointer;
    font-family: inherit;
  }
  .tool:hover:not(:disabled) {
    background: var(--mdc-card-hover);
  }
  .tool:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .tool.primary {
    background: var(--mdc-accent);
    color: var(--mdc-bg);
    border-color: var(--mdc-accent);
    font-weight: 600;
  }
  .spacer {
    flex: 1;
  }
  .status {
    font-family: var(--mdc-mono);
    font-size: 0.78rem;
    color: var(--mdc-dim);
  }
  .app-error {
    display: flex;
    justify-content: center;
    gap: 0.7rem;
    padding: 0.35rem 0.6rem;
    background: rgba(247, 118, 142, 0.14);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.78rem;
  }
  .app-error button {
    color: inherit;
    background: transparent;
    border: 1px solid currentColor;
    border-radius: 3px;
    cursor: pointer;
  }
  .layout {
    flex: 1;
    display: flex;
    flex-direction: row;
    gap: 0.5rem;
    padding: 0.5rem;
    overflow: hidden;
    min-height: 0;
  }
  .force-layout {
    flex: 1;
    display: flex;
    flex-direction: row;
    gap: 0.5rem;
    padding: 0.5rem;
    overflow: hidden;
    min-height: 0;
  }
  .force-layout.hidden {
    display: none;
  }
  .force-canvas-wrap {
    flex: 5;
    min-width: 0;
    border: 1px solid var(--mdc-border);
    border-radius: 6px;
    overflow: hidden;
    position: relative;
  }
  .force-canvas-wrap.full {
    flex: 1;
  }
  .force-editor-wrap {
    flex: 2;
    min-width: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .full-error {
    color: var(--mdc-error);
    padding: 2rem;
    font-family: var(--mdc-mono);
  }

  /* View Transitions: node-switch animation.
   *
   * The OLD snapshot stays fully visible (no fade-out). The NEW snapshot
   * fades/slides in ON TOP of the old one. This prevents any blank frame.
   */
  :global(::view-transition-old(root)) {
    animation: none;
  }
  :global(::view-transition-new(root)) {
    animation: mdc-vt-in 0.22s ease forwards;
  }

  /* Directional modifiers: up = slide from top, down = slide from bottom. */
  :global(body[data-vt-direction="up"] ::view-transition-new(root)) {
    animation: mdc-vt-in-up 0.26s cubic-bezier(0.22, 0.61, 0.36, 1) forwards;
  }
  :global(body[data-vt-direction="down"] ::view-transition-new(root)) {
    animation: mdc-vt-in-down 0.26s cubic-bezier(0.22, 0.61, 0.36, 1) forwards;
  }

  @keyframes mdc-vt-in {
    from { opacity: 0; }
    to { opacity: 1; }
  }
  @keyframes mdc-vt-in-up {
    from { opacity: 0; transform: translateY(-12px) scale(0.985); }
    to { opacity: 1; transform: translateY(0) scale(1); }
  }
  @keyframes mdc-vt-in-down {
    from { opacity: 0; transform: translateY(12px) scale(0.985); }
    to { opacity: 1; transform: translateY(0) scale(1); }
  }
</style>
