<script lang="ts">
  import { onMount } from "svelte";
  import {
    ArrowLeft,
    ArrowRight,
    BookOpenText,
    Columns3,
    Link2,
    Network,
    Plus,
    RefreshCw,
    Search,
    Unlink2,
  } from "@lucide/svelte";
  import {
    navigate,
    withViewTransition,
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
  import DepthGraph from "./components/DepthGraph.svelte";
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
  let refreshing = $state(false);

  // Top-level view state: three-column layout vs. full-screen force graph.
  let view = $state<"columns" | "force">("columns");
  let viewSwitching = $state(false);
  // Selected fnode in the force graph (drives the side editor panel).
  let forceSelectedFnode = $state<string | null>(null);
  // Increment after dependency mutations to refresh the graph data.
  let graphRevision = $state(0);
  let forceLoadRequest = 0;
  let viewRequest = 0;
  let forceRelationsDirty = false;
  // NodeDetail for the force-graph side panel (fetched on selection).
  let forceNodeLoad = $state<LoadState>({ kind: "idle" });
  let forceEditorRevision = $state(0);

  onMount(() => {
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      if (!hasUnsavedDrafts()) return;
      event.preventDefault();
      event.returnValue = "";
    };
    let restoringHistory = false;
    let popstateRequest = 0;
    // Global shortcuts: "/" opens search, "g" toggles the workspace view.
    // Ignored while typing (inputs, textareas, contenteditable e.g. CodeMirror).
    const onKeyDown = (event: KeyboardEvent) => {
      if (overlay.kind !== "none") return;
      const target = event.target;
      if (target instanceof HTMLElement) {
        const tag = target.tagName;
        if (tag === "INPUT" || tag === "TEXTAREA" || target.isContentEditable) return;
      }
      if (event.key === "/") {
        event.preventDefault();
        overlay = { kind: "search" };
      } else if (event.key.toLowerCase() === "g" &&
        !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey) {
        event.preventDefault();
        void toggleGraphView();
      }
    };
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
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("beforeunload", onBeforeUnload);
      window.removeEventListener("popstate", onPopState);
      window.removeEventListener("keydown", onKeyDown);
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
    if (refreshing) return;
    // Refresh both views so switching between them doesn't show stale data.
    refreshError = null;
    refreshing = true;
    graphRevision++;
    try {
      const [forceOk, columnOk] = await Promise.all([
        refreshForceNodeRaw(true),
        refreshCurrent(true),
      ]);
      if (!forceOk || !columnOk) {
        refreshError = "one or more refresh requests failed";
      }
    } finally {
      refreshing = false;
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
      forceNodeLoad = { kind: "ready", node };
      return true;
    } catch (e) {
      if (request === forceLoadRequest && forceSelectedFnode === targetFnode) {
        forceNodeLoad = { kind: "error", message: e instanceof Error ? e.message : String(e) };
      }
      return false;
    }
  }

  async function refreshReusedForceNode(
    targetFnode: string,
    request: number,
    draftRevision: number,
  ) {
    try {
      const node = await api.node(targetFnode);
      if (request !== forceLoadRequest || forceSelectedFnode !== targetFnode) return;
      if (unsavedDraftRevision() !== draftRevision) return;
      forceNodeLoad = { kind: "ready", node };
    } catch {
      // Keep the already displayed Knowledge snapshot if background refresh fails.
    }
  }

  function afterDepMutation() {
    graphRevision++;
    if (view === "force") {
      forceRelationsDirty = true;
      void refreshForceNodeRaw();
    }
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

  let statusText = $derived.by(() => {
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
  });

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
    const request = ++forceLoadRequest;
    if (!fnode) {
      let committed = false;
      await withViewTransition("neutral", () => {
        if (request !== forceLoadRequest) return;
        forceEditorRevision++;
        forceSelectedFnode = null;
        forceNodeLoad = { kind: "idle" };
        committed = true;
      }, "force-editor");
      return committed;
    }
    const confirmedDraftRevision = unsavedDraftRevision();
    try {
      const node = await api.node(fnode);
      if (request !== forceLoadRequest) return false;
      if (unsavedDraftRevision() !== confirmedDraftRevision) return false;
      let committed = false;
      await withViewTransition("neutral", () => {
        if (request !== forceLoadRequest || committed) return;
        if (unsavedDraftRevision() !== confirmedDraftRevision) return;
        forceEditorRevision++;
        forceSelectedFnode = fnode;
        forceNodeLoad = { kind: "ready", node };
        commitFocusedHistory(fnode, {
          pushHistory: opts.pushHistory,
          historyIndex: opts.historyIndex,
          browserHistory: opts.browserHistory,
        });
        committed = true;
      }, "force-editor");
      return committed;
    } catch (e) {
      if (request !== forceLoadRequest) return false;
      if (!opts.preserveOnFailure) {
        await withViewTransition("neutral", () => {
          if (request !== forceLoadRequest) return;
          forceEditorRevision++;
          forceSelectedFnode = fnode;
          forceNodeLoad = {
            kind: "error",
            message: e instanceof Error ? e.message : String(e),
          };
        }, "force-editor");
      }
      return false;
    }
  }

  async function toggleGraphView() {
    if (viewSwitching || !confirmDiscardDrafts()) return;
    viewSwitching = true;
    const request = ++viewRequest;
    try {
      if (view === "columns") {
        if (!await settlePendingMutations()) return;
        // Reuse the node already displayed by Knowledge. The graph data loads
        // progressively after the view changes, so switching never waits for
        // a full-workspace fetch or layout pass.
        const forceRequest = ++forceLoadRequest;
        if (appState.load.kind === "ready") {
          const node = appState.load.node;
          const draftRevision = unsavedDraftRevision();
          forceSelectedFnode = node.fnode;
          forceNodeLoad = { kind: "ready", node };
          commitFocusedHistory(node.fnode, { pushHistory: false });
          void refreshReusedForceNode(node.fnode, forceRequest, draftRevision);
        } else {
          forceSelectedFnode = null;
          forceNodeLoad = { kind: "idle" };
        }
        forceRelationsDirty = false;
        if (request !== viewRequest) return;
        view = "force";
      } else {
        // Keep the complete graph view visible until the column data is ready.
        const target = forceSelectedFnode;
        const canReuseColumns = target !== null &&
          appState.load.kind === "ready" &&
          appState.load.node.fnode === target &&
          !forceRelationsDirty;
        if (canReuseColumns && forceNodeLoad.kind === "ready") {
          refreshFocused(forceNodeLoad.node);
        } else if (target) {
          await navigate(target, {
            pushHistory: false,
            skipTransition: true,
            skipUnsavedGuard: true,
          });
        }
        if (request !== viewRequest) return;
        view = "columns";
        forceSelectedFnode = null;
        forceLoadRequest++;
        forceNodeLoad = { kind: "idle" };
        forceRelationsDirty = false;
      }
    } finally {
      if (request === viewRequest) {
        viewSwitching = false;
      }
    }
  }
</script>

<div class="app" inert={overlay.kind !== "none"}>
  <header class="toolbar">
    <div class="identity" aria-label="MathDoc">
      <span class="brand-mark"><BookOpenText size={17} strokeWidth={1.8} /></span>
      <span class="brand-copy">
        <strong>MathDoc</strong>
        <small>DAG workspace</small>
      </span>
    </div>
    <span class="toolbar-divider"></span>
    <div class="history-tools" aria-label="navigation history">
      <button
        class="tool icon-only"
        onclick={() => void goBack()}
        disabled={!canGoBack()}
        title="Back"
        aria-label="Back"
      ><ArrowLeft size={16} strokeWidth={1.8} /></button>
      <button
        class="tool icon-only"
        onclick={() => void goForward()}
        disabled={!canGoForward()}
        title="Forward"
        aria-label="Forward"
      ><ArrowRight size={16} strokeWidth={1.8} /></button>
    </div>
    <button class="tool search-tool" onclick={() => (overlay = { kind: "search" })} title="Search nodes (/)">
      <Search size={15} strokeWidth={1.9} />
      <span>Search</span>
      <kbd class="tool-kbd">/</kbd>
    </button>
    <span class="toolbar-divider compact"></span>
    <div class="node-tools" aria-label="node actions">
      <button
        class="tool"
        onclick={() => { if (activeFnode) overlay = { kind: "add-dep", target: activeFnode }; }}
        disabled={!activeReady}
        title="Add dependency"
      ><Link2 size={15} strokeWidth={1.8} /><span>Add dependency</span></button>
      <button
        class="tool"
        onclick={() => { if (activeFnode) overlay = { kind: "rm-dep", target: activeFnode }; }}
        disabled={!activeReady || activeDepens.length === 0}
        title="Remove dependency"
      ><Unlink2 size={15} strokeWidth={1.8} /><span>Remove dependency</span></button>
      <button
        class="tool"
        onclick={() => { if (activeFnode) overlay = { kind: "new-node", target: activeFnode }; }}
        disabled={!activeReady}
        title="Create node"
      ><Plus size={15} strokeWidth={2} /><span>New node</span></button>
    </div>
    <span class="toolbar-divider compact"></span>
    <div class="view-switch" aria-label="workspace view">
      <button
        class:active={view === "columns"}
        aria-pressed={view === "columns"}
        disabled={viewSwitching}
        onclick={() => { if (view !== "columns") void toggleGraphView(); }}
        title="Knowledge view"
      ><Columns3 size={15} strokeWidth={1.8} /><span>Knowledge</span></button>
      <button
        class:active={view === "force"}
        aria-pressed={view === "force"}
        disabled={viewSwitching}
        onclick={() => { if (view !== "force") void toggleGraphView(); }}
        title="Graph view"
      ><Network size={15} strokeWidth={1.8} /><span>Graph</span></button>
    </div>
    <button
      class="tool icon-only"
      class:spinning={refreshing}
      onclick={refreshView}
      disabled={refreshing}
      title="Refresh external file changes"
      aria-label="Refresh external file changes"
    ><RefreshCw size={16} strokeWidth={1.8} /></button>
    <span class="spacer"></span>
    {#if statusText}
      <span class="toolbar-divider compact"></span>
      <span class="status"><span class="status-dot"></span>{statusText}</span>
    {/if}
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
    <div class="force-canvas-wrap">
        <DepthGraph
          active={view === "force"}
          onSelect={onForceSelect}
          selectedFnode={forceSelectedFnode}
          revision={graphRevision}
        />
    </div>
    <div class="force-editor-wrap">
      {#if view === "force"}
        {#key forceEditorRevision}
          <EditorPane load={forceNodeLoad} onRefresh={refreshForceNode} />
        {/key}
      {/if}
    </div>
  </main>

  <!-- Unmount editors after a confirmed view switch so hidden drafts cannot linger. -->
  {#if view === "columns"}
  <main class="layout">
    {#if initialError}
      <div class="full-error">{initialError}</div>
    {:else}
      <NodeColumn
        title="Referrers"
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
        title="Dependencies"
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
  .app {
    display: flex;
    flex-direction: column;
    height: 100%;
    position: relative;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    min-height: 58px;
    padding: 0 0.85rem;
    border-bottom: 1px solid var(--mdc-border);
    background: rgba(15, 21, 31, 0.94);
    box-shadow: 0 1px 0 rgba(255, 255, 255, 0.015);
    flex-shrink: 0;
  }
  .identity {
    display: flex;
    align-items: center;
    gap: 0.65rem;
    min-width: 148px;
  }
  .brand-mark {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    color: #101722;
    background: linear-gradient(145deg, var(--mdc-accent-strong), #6fd4c3);
    border-radius: 9px;
    box-shadow: 0 5px 18px rgba(89, 124, 224, 0.22);
  }
  .brand-copy {
    display: flex;
    flex-direction: column;
    line-height: 1.05;
  }
  .brand-copy strong {
    color: var(--mdc-fg);
    font-size: 0.9rem;
    letter-spacing: -0.01em;
  }
  .brand-copy small {
    margin-top: 0.25rem;
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: 0.62rem;
    letter-spacing: 0.02em;
  }
  .toolbar-divider {
    width: 1px;
    height: 26px;
    margin: 0 0.25rem;
    background: var(--mdc-border);
  }
  .toolbar-divider.compact {
    margin-inline: 0.1rem;
  }
  .history-tools,
  .node-tools {
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }
  .tool {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.42rem;
    min-height: 32px;
    padding: 0 0.65rem;
    color: var(--mdc-fg-soft);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--mdc-radius-sm);
    font-size: 0.76rem;
    font-weight: 550;
    cursor: pointer;
    transition: color 120ms ease, background 120ms ease, border-color 120ms ease;
  }
  .tool:hover:not(:disabled) {
    background: var(--mdc-card-hover);
    border-color: var(--mdc-border);
    color: var(--mdc-fg);
  }
  .tool:disabled {
    opacity: 0.32;
    cursor: not-allowed;
  }
  .tool.icon-only {
    width: 32px;
    padding: 0;
  }
  .search-tool {
    padding-inline: 0.72rem 0.85rem;
    color: var(--mdc-fg);
    background: var(--mdc-card);
    border-color: var(--mdc-border);
  }
  .tool-kbd {
    min-width: 17px;
    padding: 0.1rem 0.28rem;
    color: var(--mdc-muted);
    background: var(--mdc-bg);
    border: 1px solid var(--mdc-border);
    border-radius: 4px;
    font-family: var(--mdc-mono);
    font-size: 0.6rem;
    line-height: 1.1;
    text-align: center;
  }
  .tool.spinning :global(svg) {
    animation: mdc-spin 0.8s linear infinite;
  }
  @keyframes mdc-spin {
    to { transform: rotate(360deg); }
  }
  .view-switch {
    display: flex;
    align-items: center;
    padding: 3px;
    background: var(--mdc-bg);
    border: 1px solid var(--mdc-border);
    border-radius: 8px;
  }
  .view-switch button {
    display: inline-flex;
    align-items: center;
    gap: 0.38rem;
    min-height: 27px;
    padding: 0 0.62rem;
    color: var(--mdc-muted);
    background: transparent;
    border: 0;
    border-radius: 5px;
    font-size: 0.72rem;
    font-weight: 600;
    cursor: pointer;
  }
  .view-switch button:hover {
    color: var(--mdc-fg-soft);
  }
  .view-switch button:disabled {
    cursor: wait;
  }
  .view-switch button.active {
    color: var(--mdc-fg);
    background: var(--mdc-card-selected);
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.22);
  }
  .spacer {
    flex: 1;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    max-width: 250px;
    font-family: var(--mdc-mono);
    font-size: 0.68rem;
    color: var(--mdc-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .status-dot {
    width: 6px;
    height: 6px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: var(--mdc-accent-down);
    box-shadow: 0 0 0 3px rgba(99, 216, 178, 0.09);
  }
  .app-error {
    display: flex;
    justify-content: center;
    gap: 0.7rem;
    padding: 0.45rem 0.7rem;
    background: rgba(255, 125, 143, 0.1);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.72rem;
    border-bottom: 1px solid rgba(255, 125, 143, 0.2);
  }
  .app-error button {
    color: inherit;
    background: transparent;
    border: 1px solid currentColor;
    border-radius: var(--mdc-radius-sm);
    cursor: pointer;
  }
  .layout {
    flex: 1;
    display: flex;
    flex-direction: row;
    gap: 0.75rem;
    padding: 0.75rem;
    overflow: hidden;
    min-height: 0;
  }
  .force-layout {
    flex: 1;
    display: flex;
    flex-direction: row;
    gap: 0.75rem;
    padding: 0.75rem;
    overflow: hidden;
    min-height: 0;
  }
  .force-layout.hidden {
    position: absolute;
    inset: 58px 0 0;
    visibility: hidden;
    pointer-events: none;
  }
  .force-canvas-wrap {
    flex: 5;
    min-width: 0;
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-md);
    overflow: hidden;
    position: relative;
    box-shadow: 0 10px 35px rgba(0, 0, 0, 0.16);
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

  @media (max-width: 1180px) {
    .identity {
      min-width: auto;
    }
    .brand-copy small,
    .node-tools .tool span,
    .search-tool .tool-kbd {
      display: none;
    }
    .node-tools .tool {
      width: 32px;
      padding: 0;
    }
    .status {
      max-width: 140px;
    }
  }

  @media (max-width: 940px) {
    .brand-copy,
    .status,
    .toolbar-divider.compact {
      display: none;
    }
    .identity {
      min-width: 32px;
    }
  }

  /* View Transitions: the new snapshot fades over the stable old snapshot.
     Directional slides make up/down navigation feel spatial. */
  :global(html[data-vt-scope="force-editor"]) {
    view-transition-name: none;
  }
  :global(html[data-vt-scope="force-editor"] .force-editor-wrap) {
    view-transition-name: force-editor;
    background: var(--mdc-bg);
    border-radius: var(--mdc-radius-md);
  }
  :global(::view-transition-group(force-editor)) {
    animation: none;
  }
  :global(::view-transition-image-pair(force-editor)) {
    isolation: auto;
  }
  :global(::view-transition-old(force-editor)) {
    animation: none;
    mix-blend-mode: normal;
    opacity: 1;
  }
  :global(::view-transition-new(force-editor)) {
    mix-blend-mode: normal;
    animation: mdc-vt-in 0.22s ease forwards;
  }
  :global(::view-transition-old(root)) {
    animation: none;
  }
  :global(::view-transition-new(root)) {
    animation: mdc-vt-in 0.22s ease forwards;
  }

  @keyframes mdc-vt-in {
    from { opacity: 0; }
    to { opacity: 1; }
  }
</style>
