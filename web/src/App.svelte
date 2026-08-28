<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    ArrowLeft,
    ArrowRight,
    Columns3,
    Link2,
    Moon,
    Network,
    Plus,
    RefreshCw,
    Search,
    Sun,
    Unlink2,
  } from "@lucide/svelte";
  import {
    nodeSession,
    browserHistoryEntry,
    browserHistoryTarget,
    commitClearedHistory,
    initialHistoryOptions,
    type BrowserHistoryEntry,
    type BrowserHistoryMode,
  } from "./lib/state.svelte";
  import { api } from "./lib/api";
  import NodeColumn from "./components/NodeColumn.svelte";
  import EditorPane from "./components/EditorPane.svelte";
  import SearchOverlay from "./components/SearchOverlay.svelte";
  import AddDepOverlay from "./components/AddDepOverlay.svelte";
  import RmDepOverlay from "./components/RmDepOverlay.svelte";
  import NewNodeOverlay from "./components/NewNodeOverlay.svelte";
  import DepthGraph from "./components/DepthGraph.svelte";
  import type { GraphCheckReport, NodeDetail } from "./lib/types";
  import {
    confirmDiscardDrafts,
    hasUnsavedDrafts,
    settlePendingMutations,
  } from "./lib/unsaved";
  import { applyTheme, currentTheme, observeTheme, type Theme } from "./lib/theme";

  type Overlay =
    | { kind: "none" }
    | { kind: "search" }
    | { kind: "add-dep"; target: string }
    | { kind: "rm-dep"; target: string }
    | { kind: "new-node" };

  let overlay = $state<Overlay>({ kind: "none" });
  let theme = $state<Theme>(currentTheme());
  let startupRequest = 0;
  let initialNavigationRetry: { fnode: string; clearedEntry: BrowserHistoryEntry | null } | null = null;
  let initialError = $state<string | null>(null);
  let refreshError = $state<string | null>(null);
  let refreshing = $state(false);
  let refreshRequest = 0;
  let historyNavigating = $state(false);
  let graphCheck = $state<GraphCheckReport | null>(null);
  let graphCheckLoading = $state(false);
  let graphCheckError: string | null = $state(null);
  let graphCheckStale = $state(false);
  let graphCheckRequest = 0;

  // Top-level view state: three-column layout vs. full-screen force graph.
  let view = $state<"columns" | "force">("columns");
  let viewSwitching = $state(false);
  let columnsMounted = $state(true);
  let forceEditorMounted = $state(false);
  let resolveColumnsEditorReady: (() => void) | null = null;
  let resolveForceEditorReady: (() => void) | null = null;
  // Increment after dependency mutations to refresh the graph data.
  let graphRevision = $state(0);
  let viewSwitchDone: Promise<void> | null = null;
  let graphIssueCount = $derived(
    graphCheck
      ? graphCheck.missing.length + graphCheck.invalid.length + graphCheck.cycles.length
      : 0,
  );
  let graphCheckTitle = $derived.by(() => {
    if (graphCheckError) return graphCheckError;
    if (graphCheckStale) return "Graph counts updated locally; refresh to recheck issues";
    if (!graphCheck) return "Checking graph";
    return graphIssueCount === 0
      ? "Graph check: no issues"
      : `Graph check: ${graphIssueCount} issue${graphIssueCount === 1 ? "" : "s"}`;
  });

  async function refreshGraphCheck(refreshWorkspace = false): Promise<boolean> {
    const request = ++graphCheckRequest;
    graphCheckLoading = true;
    graphCheckError = null;
    try {
      const report = await (refreshWorkspace ? api.refreshWorkspace() : api.graphCheck());
      if (request !== graphCheckRequest) return false;
      graphCheck = report;
      graphCheckStale = false;
      return true;
    } catch (error) {
      if (request === graphCheckRequest) {
        graphCheckError = error instanceof Error ? error.message : String(error);
      }
      return false;
    } finally {
      if (request === graphCheckRequest) graphCheckLoading = false;
    }
  }

  function cancelStartup() {
    startupRequest++;
  }

  function toggleTheme() {
    theme = theme === "dark" ? "light" : "dark";
    applyTheme(theme);
  }

  onMount(() => observeTheme((nextTheme) => {
    theme = nextTheme;
    applyTheme(nextTheme, false);
  }));

  async function navigateInitial(
    fnode: string,
    clearedEntry: BrowserHistoryEntry | null = null,
  ): Promise<boolean> {
    const committed = await nodeSession.select(fnode, initialHistoryOptions(fnode));
    if (committed && clearedEntry) {
      commitClearedHistory({
        pushHistory: false,
        historyIndex: clearedEntry.index,
        historyEntries: clearedEntry.entries,
        browserHistory: "replace",
      });
    }
    return committed;
  }

  function applyGraphStatsDelta(nodes: number, edges: number) {
    graphCheckRequest++;
    graphCheckLoading = false;
    graphCheckError = null;
    if (!graphCheck) {
      void refreshGraphCheck();
      return;
    }
    graphCheck = {
      ...graphCheck,
      nodes: Math.max(0, graphCheck.nodes + nodes),
      edges: Math.max(0, graphCheck.edges + edges),
    };
    graphCheckStale = true;
  }

  function markColumnsEditorReady() {
    const resolve = resolveColumnsEditorReady;
    resolveColumnsEditorReady = null;
    resolve?.();
  }

  function markForceEditorReady() {
    const resolve = resolveForceEditorReady;
    resolveForceEditorReady = null;
    resolve?.();
  }

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
      if (overlay.kind !== "none" || viewSwitching || refreshing || historyNavigating) return;
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
      const request = ++popstateRequest;
      cancelStartup();
      nodeSession.cancel();
      refreshRequest++;
      if (restoringHistory) {
        restoringHistory = false;
        historyNavigating = false;
        return;
      }
      const entry = browserHistoryEntry(event.state);
      if (!entry) return;

      historyNavigating = true;
      let awaitingRollback = false;
      try {
        const pendingViewSwitch = viewSwitchDone;
        if (pendingViewSwitch) await pendingViewSwitch;
        if (request !== popstateRequest) return;
        const activeEntry = browserHistoryEntry(window.history.state);
        if (!activeEntry || activeEntry.index !== entry.index || activeEntry.fnode !== entry.fnode) return;
        const previousIndex = nodeSession.historyIdx;
        const target = browserHistoryTarget(entry);
        const committed = view === "force"
          ? await onForceSelect(entry.fnode, {
              pushHistory: false,
              historyIndex: entry.index,
              historyEntries: entry.entries,
              browserHistory: "replace",
            })
          : await nodeSession.select(target, {
              pushHistory: false,
              historyIndex: entry.index,
              historyEntries: entry.entries,
              browserHistory: "replace",
            });
        if (request !== popstateRequest) return;
        if (committed) {
          if (view === "columns" && entry.fnode === null) {
            commitClearedHistory({
              pushHistory: false,
              historyIndex: entry.index,
              historyEntries: entry.entries,
              browserHistory: "replace",
            });
          }
          overlay = { kind: "none" };
          return;
        }
        const currentEntry = browserHistoryEntry(window.history.state);
        if (!currentEntry || currentEntry.index !== entry.index ||
          currentEntry.fnode !== entry.fnode || nodeSession.historyIdx !== previousIndex) return;
        const delta = previousIndex - entry.index;
        if (delta !== 0) {
          restoringHistory = true;
          awaitingRollback = true;
          window.history.go(delta);
        }
      } finally {
        if (request === popstateRequest && !awaitingRollback) historyNavigating = false;
      }
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    window.addEventListener("popstate", onPopState);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      graphCheckRequest++;
      resolveColumnsEditorReady?.();
      resolveForceEditorReady?.();
      window.removeEventListener("beforeunload", onBeforeUnload);
      window.removeEventListener("popstate", onPopState);
      window.removeEventListener("keydown", onKeyDown);
    };
  });

  // Pick a default starting node on mount: deepest root, else first.
  onMount(() => {
    void (async () => {
      const request = ++startupRequest;
      const isCurrent = () => request === startupRequest;
      try {
        // URL hash can override the default even when a cyclic graph has no roots.
        const hash = window.location.hash.slice(1);
        const params = new URLSearchParams(hash);
        const currentEntry = browserHistoryEntry(window.history.state);
        const hashRef = params.get("ref");
        const clearedEntry = hashRef === null && currentEntry?.fnode === null ? currentEntry : null;
        const ref = hashRef ??
          (currentEntry?.fnode === null ? browserHistoryTarget(currentEntry) : null);
        if (ref) {
          const resolved = await api.resolve(ref);
          if (!isCurrent()) return;
          const committed = await navigateInitial(resolved.fnode, clearedEntry);
          if (isCurrent() && !committed) {
            initialNavigationRetry = { fnode: resolved.fnode, clearedEntry };
          }
          return;
        }
        const roots = (await api.roots()).filter((node) => !node.broken);
        if (!isCurrent()) return;
        if (roots.length === 0) {
          const graph = await api.full();
          if (!isCurrent()) return;
          const fallback = graph.nodes[0];
          if (!fallback) {
            initialError = "workspace has no valid nodes — create one with New node";
            return;
          }
          const committed = await navigateInitial(fallback.fnode);
          if (isCurrent() && !committed) {
            initialNavigationRetry = { fnode: fallback.fnode, clearedEntry: null };
          }
          return;
        }
        const deepest = [...roots].sort((a, b) => b.topo_depth - a.topo_depth)[0]!;
        const committed = await navigateInitial(deepest.fnode);
        if (isCurrent() && !committed) {
          initialNavigationRetry = { fnode: deepest.fnode, clearedEntry: null };
        }
      } catch (e) {
        if (isCurrent()) initialError = e instanceof Error ? e.message : String(e);
      } finally {
        void refreshGraphCheck();
      }
    })();
  });

  $effect(() => {
    if (nodeSession.load.kind === "ready") {
      initialError = null;
      initialNavigationRetry = null;
    }
  });

  async function refreshView() {
    if (refreshing) return;
    if (!confirmDiscardDrafts()) return;
    const request = ++refreshRequest;
    refreshError = null;
    refreshing = true;
    try {
      if (!await settlePendingMutations()) return;
      if (request !== refreshRequest) return;
      const checked = await refreshGraphCheck(true);
      if (request !== refreshRequest) return;
      const selectionWasCleared = nodeSession.selectionCleared;
      const current = nodeSession.node;
      const refreshed = !current || await nodeSession.select(
        current.fnode,
        {
          pushHistory: false,
          skipTransition: true,
          skipUnsavedGuard: true,
        },
      );
      if (selectionWasCleared && view === "force") nodeSession.selectionCleared = true;
      if (request !== refreshRequest) return;
      if (refreshed || checked) graphRevision++;
      if (!refreshed) refreshError = "refresh request failed";
    } finally {
      refreshing = false;
    }
  }

  function refreshNode(node: NodeDetail, graphChanged = false) {
    if (graphChanged) graphRevision++;
    nodeSession.acceptNode(node);
  }

  function afterDepMutation(updated: NodeDetail, delta: { nodes: number; edges: number }) {
    refreshError = null;
    graphRevision++;
    applyGraphStatsDelta(delta.nodes, delta.edges);
    nodeSession.acceptNode(updated);
    void nodeSession.syncView(updated.fnode).catch((error) => {
      refreshError = error instanceof Error ? error.message : String(error);
    });
  }

  function afterNodeCreated(fnode: string, skipUnsavedGuard = false) {
    cancelStartup();
    initialError = null;
    graphRevision++;
    applyGraphStatsDelta(1, 0);
    if (historyNavigating) return;
    if (view === "force") void onForceSelect(fnode, { skipUnsavedGuard });
    else void nodeSession.select(fnode, { skipUnsavedGuard });
  }

  // The fnode that toolbar actions operate on, regardless of view.
  let activeFnode = $derived(
    view === "force" ? nodeSession.selectedFnode : nodeSession.node?.fnode ?? null,
  );
  let activeNode = $derived(
    view === "force" && nodeSession.selectionCleared ? null : nodeSession.node,
  );
  // Whether the active node is editable (non-broken).
  let activeReady = $derived(activeFnode !== null && activeNode !== null && !activeNode.broken);
  let activeRevision = $derived(activeNode?.revision ?? null);
  // Depens of the active node.
  let activeDepens = $derived(activeNode?.depens ?? []);

  $effect(() => {
    if ("target" in overlay && activeFnode !== overlay.target) {
      overlay = { kind: "none" };
    }
  });

  let statusText = $derived.by(() => {
    const s = view === "force" ? nodeSession.selectedLoad : nodeSession.load;
    if (s.kind === "ready") {
      return `${s.node.title}  ·  ${s.node.fnode.slice(0, 8)}`;
    }
    if (s.kind === "error") return `error: ${s.message}`;
    return "";
  });

  async function onForceSelect(
    fnode: string | null,
    opts: {
      skipUnsavedGuard?: boolean;
      pushHistory?: boolean;
      historyIndex?: number;
      historyEntries?: string[];
      browserHistory?: BrowserHistoryMode;
    } = {},
  ): Promise<boolean> {
    cancelStartup();
    return fnode ? nodeSession.select(fnode, opts) : nodeSession.clearSelection(opts);
  }

  async function toggleGraphView() {
    if (viewSwitching || refreshing || historyNavigating || !confirmDiscardDrafts()) return;
    cancelStartup();
    viewSwitching = true;
    nodeSession.cancel();
    let resolveViewSwitch: () => void;
    const switchDone = new Promise<void>((resolve) => {
      resolveViewSwitch = resolve;
    });
    viewSwitchDone = switchDone;
    try {
      if (!await settlePendingMutations()) return;
      if (view === "columns") {
        if (nodeSession.node) {
          const entry = browserHistoryEntry(window.history.state);
          const selectionWasCleared = entry?.fnode === null &&
            browserHistoryTarget(entry) === nodeSession.node.fnode;
          nodeSession.selectionCleared = selectionWasCleared;
        }
        const editorReady = new Promise<void>((resolve) => {
          resolveForceEditorReady = resolve;
        });
        forceEditorMounted = true;
        await tick();
        await editorReady;
        view = "force";
        await tick();
        columnsMounted = false;
        await tick();
      } else {
        // The backing NodeView remains loaded when graph selection is cleared.
        nodeSession.selectionCleared = false;
        const editorReady = new Promise<void>((resolve) => {
          resolveColumnsEditorReady = resolve;
        });
        columnsMounted = true;
        await tick();
        await editorReady;
        view = "columns";
        await tick();
        forceEditorMounted = false;
      }
    } finally {
      resolveViewSwitch!();
      if (viewSwitchDone === switchDone) viewSwitchDone = null;
      viewSwitching = false;
    }
  }
</script>

<div
  class="app"
  inert={viewSwitching || refreshing || historyNavigating}
  aria-busy={viewSwitching || refreshing || historyNavigating}
>
  <header class="toolbar">
    <div class="identity" aria-label="MathDoc">
      <img class="brand-mark" src="/mdc-logo.svg" alt="" />
      <span class="brand-copy">
        <strong>MathDoc</strong>
      </span>
    </div>
    <span class="toolbar-divider"></span>
    <div class="history-tools" aria-label="navigation history">
      <button
        class="tool icon-only"
        onclick={() => window.history.back()}
        disabled={nodeSession.historyIdx <= 0}
        title="Back"
        aria-label="Back"
      ><ArrowLeft size={16} strokeWidth={1.8} /></button>
      <button
        class="tool icon-only"
        onclick={() => window.history.forward()}
        disabled={nodeSession.historyIdx >= nodeSession.history.length - 1}
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
        onclick={() => { overlay = { kind: "new-node" }; }}
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
      onclick={toggleTheme}
      title={`Switch to ${theme === "dark" ? "light" : "dark"} mode`}
      aria-label={`Switch to ${theme === "dark" ? "light" : "dark"} mode`}
    >
      {#if theme === "dark"}<Sun size={16} strokeWidth={1.8} />
      {:else}<Moon size={16} strokeWidth={1.8} />{/if}
    </button>
    <button
      class="tool icon-only"
      class:spinning={refreshing}
      onclick={refreshView}
      disabled={refreshing}
      title="Refresh external file changes"
      aria-label="Refresh external file changes"
    ><RefreshCw size={16} strokeWidth={1.8} /></button>
    <span
      class="graph-stats"
      class:checking={graphCheckLoading}
      class:stale={graphCheckStale}
      class:issues={graphIssueCount > 0}
      class:error={graphCheckError !== null}
      title={graphCheckTitle}
      aria-live="polite"
    >
      <span class="graph-stats-dot"></span>
      {#if graphCheck}
        {graphCheck.nodes.toLocaleString()} nodes&nbsp;&nbsp;{graphCheck.edges.toLocaleString()} edges
      {:else if graphCheckLoading}
        Checking graph…
      {:else}
        Graph check unavailable
      {/if}
    </span>
    <span class="spacer"></span>
    {#if statusText}
      <span class="toolbar-divider compact"></span>
      <span class="status"><span class="status-dot"></span>{statusText}</span>
    {/if}
  </header>
  {#if nodeSession.navigationError || refreshError}
    <div class="app-error" role="alert">
      <span>{nodeSession.navigationError ?? refreshError}</span>
      <button onclick={() => {
        if (nodeSession.failedNavigationFnode) {
          const target = nodeSession.failedNavigationFnode;
          const initialRetry = initialNavigationRetry?.fnode === target
            ? initialNavigationRetry
            : null;
          if (initialRetry) {
            cancelStartup();
            if (view === "force" && !initialRetry.clearedEntry) {
              void onForceSelect(target, initialHistoryOptions(target));
            } else {
              void navigateInitial(target, initialRetry.clearedEntry);
            }
          } else if (view === "force") {
            void onForceSelect(target);
          } else {
            const retryingCurrent = nodeSession.load.kind === "ready" &&
              nodeSession.load.node.fnode === target;
            cancelStartup();
            void nodeSession.select(target, { pushHistory: !retryingCurrent });
          }
        } else {
          void refreshView();
        }
      }}>retry</button>
    </div>
  {/if}

  <!-- Force graph view: always mounted, hidden via CSS when in columns mode. -->
  <main
    class="force-layout"
    class:hidden={view !== "force"}
    inert={view !== "force"}
    aria-hidden={view !== "force"}
  >
    <div class="force-canvas-wrap">
        <DepthGraph
          active={view === "force"}
          {theme}
          onSelect={onForceSelect}
          selectedFnode={nodeSession.selectedFnode}
          revision={graphRevision}
        />
    </div>
    <div class="force-editor-wrap">
      {#if forceEditorMounted}
        {#key nodeSession.editorRevision}
          <EditorPane
            load={nodeSession.selectedLoad}
            {theme}
            active={view === "force"}
            onRefresh={refreshNode}
            onReady={markForceEditorReady}
          />
        {/key}
      {/if}
    </div>
  </main>

  <!-- Pre-mount the destination editor while transparent, then discard the source. -->
  {#if columnsMounted}
  <main
    class="layout"
    class:hidden={view !== "columns"}
    inert={view !== "columns"}
    aria-hidden={view !== "columns"}
  >
    {#if initialError}
      <div class="full-error">{initialError}</div>
    {:else}
      <NodeColumn
        title="Referrers"
        items={nodeSession.referrers.items}
        selected={nodeSession.referrers.selected}
        accent="up"
        lastVisitedFnode={nodeSession.lastVisitedFnode}
        onSelect={(fnode) => { cancelStartup(); return nodeSession.select(fnode, { direction: "up" }); }}
        onHover={(i) => (nodeSession.referrersSelected = i)}
      />
      {#key nodeSession.editorRevision}
        <EditorPane
          load={nodeSession.load}
          {theme}
          active={view === "columns"}
          onRefresh={refreshNode}
          onReady={markColumnsEditorReady}
        />
      {/key}
      <NodeColumn
        title="Dependencies"
        items={nodeSession.children.items}
        selected={nodeSession.children.selected}
        accent="down"
        lastVisitedFnode={nodeSession.lastVisitedFnode}
        onSelect={(fnode) => { cancelStartup(); return nodeSession.select(fnode, { direction: "down" }); }}
        onHover={(i) => (nodeSession.childrenSelected = i)}
      />
    {/if}
  </main>
  {/if}
</div>

<div class="overlay-layer" inert={historyNavigating}>
{#if overlay.kind === "search"}
  <SearchOverlay
    disabled={historyNavigating}
    onPick={(fnode) => {
      if (historyNavigating) return;
      overlay = { kind: "none" };
      if (view === "force") {
        void onForceSelect(fnode);
      } else {
        cancelStartup();
        void nodeSession.select(fnode, { direction: "neutral" });
      }
    }}
    onClose={() => (overlay = { kind: "none" })}
  />
{/if}

{#if overlay.kind === "add-dep"}
  {#key overlay.target}
    <AddDepOverlay
      disabled={historyNavigating}
      targetFnode={overlay.target}
      targetRevision={activeRevision!}
      onAdded={afterDepMutation}
      onClose={() => (overlay = { kind: "none" })}
    />
  {/key}
{/if}

{#if overlay.kind === "rm-dep"}
  {#key overlay.target}
    <RmDepOverlay
      disabled={historyNavigating}
      targetFnode={overlay.target}
      targetRevision={activeRevision!}
      children={nodeSession.children.items}
      onRemoved={afterDepMutation}
      onClose={() => (overlay = { kind: "none" })}
    />
  {/key}
{/if}

{#if overlay.kind === "new-node"}
  <NewNodeOverlay
    disabled={historyNavigating}
    onCreated={afterNodeCreated}
    onClose={() => (overlay = { kind: "none" })}
  />
{/if}
</div>

<style>
  .overlay-layer { display: contents; }
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
    background: color-mix(in srgb, var(--mdc-panel) 94%, transparent);
    box-shadow: 0 1px 0 color-mix(in srgb, var(--mdc-fg) 4%, transparent);
    flex-shrink: 0;
  }
  .identity {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    min-width: 124px;
  }
  .brand-mark {
    display: block;
    width: 2.1rem;
    height: 2.1rem;
    flex: 0 0 auto;
  }
  .brand-copy {
    display: block;
    line-height: 1;
  }
  .brand-copy strong {
    color: var(--mdc-fg);
    font-size: 0.95rem;
    font-weight: 720;
    letter-spacing: -0.025em;
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
    box-shadow: 0 1px 4px color-mix(in srgb, var(--mdc-fg) 18%, transparent);
  }
  .spacer {
    flex: 1;
  }
  .graph-stats {
    display: inline-flex;
    align-items: center;
    gap: 0.42rem;
    min-height: 28px;
    padding: 0 0.55rem;
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: 0.66rem;
    white-space: nowrap;
  }
  .graph-stats-dot {
    width: 6px;
    height: 6px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: var(--mdc-accent-down);
    box-shadow: 0 0 0 3px rgba(99, 216, 178, 0.09);
  }
  .graph-stats.checking .graph-stats-dot { animation: mdc-pulse 1s ease-in-out infinite; }
  .graph-stats.issues .graph-stats-dot {
    background: var(--mdc-warning);
    box-shadow: 0 0 0 3px rgba(232, 184, 109, 0.1);
  }
  .graph-stats.stale .graph-stats-dot {
    background: var(--mdc-muted);
    box-shadow: 0 0 0 3px rgba(135, 147, 165, 0.09);
  }
  .graph-stats.error .graph-stats-dot {
    background: var(--mdc-error);
    box-shadow: 0 0 0 3px rgba(255, 125, 143, 0.1);
  }
  @keyframes mdc-pulse {
    50% { opacity: 0.35; }
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
    background: var(--mdc-panel);
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.72rem;
    border-bottom: 1px solid color-mix(in srgb, var(--mdc-error) 28%, transparent);
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
  .force-layout.hidden,
  .layout.hidden {
    position: absolute;
    inset: 58px 0 0;
    opacity: 0;
    pointer-events: none;
  }
  .force-canvas-wrap {
    flex: 1 1 auto;
    min-width: 0;
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-md);
    overflow: hidden;
    position: relative;
    box-shadow: 0 10px 35px color-mix(in srgb, var(--mdc-fg) 12%, transparent);
  }
  .force-editor-wrap {
    flex: 0 0 calc((100% - 0.75rem) / 3);
    width: calc((100% - 0.75rem) / 3);
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
    .graph-stats {
      padding-inline: 0.2rem;
    }
  }

  @media (max-width: 940px) {
    .brand-copy,
    .graph-stats,
    .status,
    .toolbar-divider.compact {
      display: none;
    }
    .identity {
      min-width: 32px;
    }
  }

  @media (max-width: 700px) {
    .force-layout {
      flex-direction: column;
    }
    .force-canvas-wrap {
      flex: 1 1 52%;
      min-height: 240px;
    }
    .force-editor-wrap {
      flex: 1 1 48%;
      width: 100%;
      min-width: 0;
      min-height: 0;
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
