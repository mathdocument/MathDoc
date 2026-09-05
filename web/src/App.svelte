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
    type BrowserHistoryEntry,
    type FocusedHistoryOptions,
  } from "./lib/state.svelte";
  import { WorkspaceSession } from "./lib/workspace.svelte";
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
  const workspaceSession = new WorkspaceSession();

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

  async function findDefaultFnode(): Promise<string | null> {
    const roots = (await api.roots()).filter((node) => !node.broken);
    if (roots.length > 0) {
      return roots.sort((a, b) => b.topo_depth - a.topo_depth)[0]!.fnode;
    }
    return (await api.full()).nodes[0]?.fnode ?? null;
  }

  async function navigateInitial(
    fnode: string,
    clearedEntry: BrowserHistoryEntry | null = null,
  ): Promise<boolean> {
    const committed = await nodeSession.select(fnode, nodeSession.initialHistoryOptions(fnode));
    if (committed && clearedEntry) {
      nodeSession.commitClearedHistory({
        pushHistory: false,
        historyIndex: clearedEntry.index,
        historyEntries: clearedEntry.entries,
        browserHistory: "replace",
      });
    }
    return committed;
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
            nodeSession.commitClearedHistory({
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
      workspaceSession.cancel();
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
        const defaultFnode = await findDefaultFnode();
        if (!isCurrent()) return;
        if (!defaultFnode) {
          initialError = "workspace has no valid nodes — create one with New node";
          return;
        }
        const committed = await navigateInitial(defaultFnode);
        if (isCurrent() && !committed) {
          initialNavigationRetry = { fnode: defaultFnode, clearedEntry: null };
        }
      } catch (e) {
        if (isCurrent()) initialError = e instanceof Error ? e.message : String(e);
      } finally {
        void workspaceSession.refresh();
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
      const checked = await workspaceSession.refresh(true);
      if (request !== refreshRequest) return;
      const selectionWasCleared = nodeSession.selectionCleared;
      const current = nodeSession.node;
      let refreshed = false;
      if (current) {
        refreshed = await nodeSession.select(current.fnode, {
          pushHistory: false,
          skipTransition: true,
          skipUnsavedGuard: true,
          clearOnError: true,
        });
        if (!refreshed && !nodeSession.node) {
          const defaultFnode = await findDefaultFnode();
          if (request !== refreshRequest) return;
          if (defaultFnode) {
            refreshed = await nodeSession.select(defaultFnode, {
              skipTransition: true,
              skipUnsavedGuard: true,
              browserHistory: "replace",
            });
          } else {
            initialError = "workspace has no valid nodes — create one with New node";
          }
        }
      } else {
        const defaultFnode = await findDefaultFnode();
        if (request !== refreshRequest) return;
        if (defaultFnode) {
          refreshed = await nodeSession.select(defaultFnode, {
            skipTransition: true,
            skipUnsavedGuard: true,
          });
        } else {
          initialError = "workspace has no valid nodes — create one with New node";
        }
      }
      if (selectionWasCleared && view === "force") nodeSession.selectionCleared = true;
      if (request !== refreshRequest) return;
      if (refreshed || checked) graphRevision++;
      if (!refreshed) refreshError = "refresh request failed";
    } catch (error) {
      if (request === refreshRequest) {
        refreshError = error instanceof Error ? error.message : String(error);
      }
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
    workspaceSession.applyDelta(delta.nodes, delta.edges);
    nodeSession.acceptNode(updated);
    void nodeSession.syncView(updated.fnode).catch((error) => {
      refreshError = error instanceof Error ? error.message : String(error);
    });
  }

  function afterNodeCreated(fnode: string, skipUnsavedGuard = false) {
    cancelStartup();
    initialError = null;
    graphRevision++;
    workspaceSession.applyDelta(1, 0);
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
    opts: FocusedHistoryOptions & { skipUnsavedGuard?: boolean } = {},
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
        if (initialError) markColumnsEditorReady();
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
    <!-- Zone 1: identity and navigation history. -->
    <div class="bar-zone bar-start">
      <div class="identity" aria-label="MathDoc">
        <img class="brand-mark" src="/mdc-logo.svg" alt="" />
        <span class="brand-copy">
          <strong>MathDoc</strong>
        </span>
      </div>
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
    </div>

    <!-- Zone 2: the command affordance, styled as a field rather than a button. -->
    <div class="bar-zone bar-center">
      <button class="search-tool" onclick={() => (overlay = { kind: "search" })} title="Search nodes (/)">
        <Search size={15} strokeWidth={1.9} />
        <span class="search-label">Search nodes</span>
        <kbd class="tool-kbd">/</kbd>
      </button>
    </div>

    <!-- Zone 3: actions, ordered secondary → primary, then view and app controls. -->
    <div class="bar-zone bar-end">
      <div class="tool-cluster" aria-label="dependency actions">
        <button
          class="tool icon-only"
          onclick={() => { if (activeFnode) overlay = { kind: "add-dep", target: activeFnode }; }}
          disabled={!activeReady}
          title="Add dependency"
          aria-label="Add dependency"
        ><Link2 size={15} strokeWidth={1.8} /></button>
        <button
          class="tool icon-only"
          onclick={() => { if (activeFnode) overlay = { kind: "rm-dep", target: activeFnode }; }}
          disabled={!activeReady || activeDepens.length === 0}
          title="Remove dependency"
          aria-label="Remove dependency"
        ><Unlink2 size={15} strokeWidth={1.8} /></button>
      </div>
      <button
        class="tool primary"
        onclick={() => { overlay = { kind: "new-node" }; }}
        title="Create node"
      ><Plus size={15} strokeWidth={2.1} /><span>New node</span></button>
      <span class="toolbar-divider"></span>
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
      <span class="toolbar-divider"></span>
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
    </div>
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
              void onForceSelect(target, nodeSession.initialHistoryOptions(target));
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
        onSelect={(fnode) => { cancelStartup(); return nodeSession.select(fnode); }}
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
        onSelect={(fnode) => { cancelStartup(); return nodeSession.select(fnode); }}
        onHover={(i) => (nodeSession.childrenSelected = i)}
      />
    {/if}
  </main>
  {/if}

  <!-- Ambient state lives here instead of competing with actions in the header. -->
  <footer class="statusbar">
    {#if statusText}
      <span class="status"><span class="status-dot"></span>{statusText}</span>
    {/if}
    <span class="spacer"></span>
    <span
      class="graph-stats"
      class:checking={workspaceSession.loading}
      class:stale={workspaceSession.stale}
      class:issues={workspaceSession.issueCount > 0}
      class:error={workspaceSession.error !== null}
      title={workspaceSession.title}
      aria-live="polite"
    >
      <span class="graph-stats-dot"></span>
      {#if workspaceSession.report}
        {workspaceSession.report.nodes.toLocaleString()} nodes · {workspaceSession.report.edges.toLocaleString()} edges
      {:else if workspaceSession.loading}
        Checking graph…
      {:else}
        Graph check unavailable
      {/if}
    </span>
    <span class="statusbar-divider"></span>
    <span class="key-hints" aria-hidden="true">
      <span><kbd>/</kbd>search</span>
      <span><kbd>g</kbd>graph</span>
    </span>
  </footer>
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
        void nodeSession.select(fnode);
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
  /* Three-zone header: identity, command field, actions. The centre zone is
     free to grow so the search field stays optically centred. */
  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    min-height: var(--mdc-toolbar-h);
    padding: 0 0.75rem;
    border-bottom: 1px solid var(--mdc-border);
    background: color-mix(in srgb, var(--mdc-panel) 82%, transparent);
    backdrop-filter: blur(12px) saturate(160%);
    flex-shrink: 0;
    z-index: 4;
  }
  .bar-zone {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
  }
  .bar-start,
  .bar-end {
    flex: 0 0 auto;
  }
  .bar-center {
    flex: 1 1 auto;
    justify-content: center;
  }
  .identity {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-right: 0.15rem;
  }
  .brand-mark {
    display: block;
    width: 1.65rem;
    height: 1.65rem;
    flex: 0 0 auto;
  }
  .brand-copy {
    display: block;
    line-height: 1;
  }
  .brand-copy strong {
    color: var(--mdc-fg);
    font-size: var(--mdc-text-md);
    font-weight: 680;
    letter-spacing: -0.03em;
  }
  .toolbar-divider {
    width: 1px;
    height: 20px;
    margin: 0 0.15rem;
    flex: 0 0 auto;
    background: var(--mdc-border);
  }
  .history-tools {
    display: flex;
    align-items: center;
    gap: 0.15rem;
  }
  .tool {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    min-height: 32px;
    padding: 0 0.7rem;
    color: var(--mdc-dim);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--mdc-radius-sm);
    font-size: var(--mdc-text-sm);
    font-weight: 550;
    cursor: pointer;
    transition: color var(--mdc-dur-fast) var(--mdc-ease),
      background var(--mdc-dur-fast) var(--mdc-ease),
      box-shadow var(--mdc-dur-fast) var(--mdc-ease);
  }
  .tool:hover:not(:disabled) {
    background: var(--mdc-card-hover);
    color: var(--mdc-fg);
  }
  .tool:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }
  .tool.icon-only {
    width: 32px;
    padding: 0;
  }
  /* The single accent action in the shell. */
  .tool.primary {
    color: var(--mdc-on-accent);
    background: var(--mdc-accent);
    font-weight: 620;
    box-shadow: var(--mdc-shadow-sm);
  }
  .tool.primary:hover:not(:disabled) {
    background: var(--mdc-accent-strong);
    color: var(--mdc-on-accent);
    box-shadow: var(--mdc-shadow-md);
  }
  /* Related icon actions share one recessed well instead of floating apart. */
  .tool-cluster {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 2px;
    background: color-mix(in srgb, var(--mdc-bg) 55%, transparent);
    border: 1px solid var(--mdc-border);
    border-radius: 10px;
  }
  .tool-cluster .tool.icon-only {
    min-height: 26px;
    width: 28px;
    border-radius: 6px;
  }
  /* Reads as an input, behaves as a command trigger. */
  .search-tool {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    width: min(360px, 100%);
    min-height: 32px;
    padding: 0 0.4rem 0 0.65rem;
    color: var(--mdc-muted);
    background: color-mix(in srgb, var(--mdc-bg) 58%, transparent);
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-sm);
    font-size: var(--mdc-text-sm);
    font-weight: 450;
    cursor: pointer;
    transition: border-color var(--mdc-dur-fast) var(--mdc-ease),
      background var(--mdc-dur-fast) var(--mdc-ease),
      color var(--mdc-dur-fast) var(--mdc-ease);
  }
  .search-tool:hover {
    color: var(--mdc-fg-soft);
    background: var(--mdc-card);
    border-color: var(--mdc-border-strong);
  }
  .search-label {
    flex: 1;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tool-kbd {
    min-width: 18px;
    padding: 0.1rem 0.3rem;
    color: var(--mdc-muted);
    background: color-mix(in srgb, var(--mdc-fg) 6%, transparent);
    border-radius: 5px;
    font-family: var(--mdc-mono);
    font-size: 0.62rem;
    line-height: 1.2;
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
    gap: 2px;
    padding: 2px;
    background: color-mix(in srgb, var(--mdc-bg) 55%, transparent);
    border: 1px solid var(--mdc-border);
    border-radius: 10px;
  }
  .view-switch button {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    min-height: 26px;
    padding: 0 0.6rem;
    color: var(--mdc-muted);
    background: transparent;
    border: 0;
    border-radius: 6px;
    font-size: var(--mdc-text-xs);
    font-weight: 600;
    cursor: pointer;
    transition: color var(--mdc-dur-fast) var(--mdc-ease),
      background var(--mdc-dur-fast) var(--mdc-ease);
  }
  .view-switch button:hover:not(.active) {
    color: var(--mdc-fg-soft);
  }
  .view-switch button:disabled {
    cursor: wait;
  }
  .view-switch button.active {
    color: var(--mdc-fg);
    background: var(--mdc-panel-raised);
    box-shadow: var(--mdc-shadow-sm);
  }
  .spacer {
    flex: 1;
  }

  /* Slim ambient state strip along the bottom of the shell. */
  .statusbar {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-shrink: 0;
    min-height: var(--mdc-statusbar-h);
    padding: 0 0.75rem;
    border-top: 1px solid var(--mdc-border);
    background: color-mix(in srgb, var(--mdc-panel) 74%, transparent);
    color: var(--mdc-muted);
    font-size: var(--mdc-text-2xs);
    z-index: 4;
  }
  .statusbar-divider {
    width: 1px;
    height: 12px;
    flex: 0 0 auto;
    background: var(--mdc-border);
  }
  .graph-stats {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--mdc-muted);
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-2xs);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .graph-stats-dot {
    width: 6px;
    height: 6px;
    flex: 0 0 auto;
    border-radius: 50%;
    background: var(--mdc-accent-down);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--mdc-accent-down) 16%, transparent);
  }
  .graph-stats.checking .graph-stats-dot { animation: mdc-pulse 1s ease-in-out infinite; }
  .graph-stats.issues .graph-stats-dot {
    background: var(--mdc-warning);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--mdc-warning) 16%, transparent);
  }
  .graph-stats.stale .graph-stats-dot {
    background: var(--mdc-muted);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--mdc-muted) 16%, transparent);
  }
  .graph-stats.error .graph-stats-dot {
    background: var(--mdc-error);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--mdc-error) 16%, transparent);
  }
  @keyframes mdc-pulse {
    50% { opacity: 0.35; }
  }
  .status {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    min-width: 0;
    max-width: 46ch;
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-2xs);
    color: var(--mdc-dim);
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
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--mdc-accent-down) 16%, transparent);
  }
  .key-hints {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex: 0 0 auto;
    color: var(--mdc-muted);
  }
  .key-hints span {
    display: inline-flex;
    align-items: center;
    gap: 0.32rem;
  }
  .key-hints kbd {
    min-width: 15px;
    padding: 0.05rem 0.25rem;
    color: var(--mdc-dim);
    background: color-mix(in srgb, var(--mdc-fg) 7%, transparent);
    border-radius: 4px;
    font-family: var(--mdc-mono);
    font-size: 0.6rem;
    text-align: center;
  }
  .app-error {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.7rem;
    padding: 0.5rem 0.75rem;
    background: color-mix(in srgb, var(--mdc-error) 12%, var(--mdc-panel));
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-xs);
    border-bottom: 1px solid color-mix(in srgb, var(--mdc-error) 30%, transparent);
  }
  .app-error button {
    min-height: 24px;
    padding: 0 0.55rem;
    color: inherit;
    background: color-mix(in srgb, var(--mdc-error) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--mdc-error) 40%, transparent);
    border-radius: 6px;
    cursor: pointer;
  }
  .app-error button:hover {
    background: color-mix(in srgb, var(--mdc-error) 22%, transparent);
  }
  .layout,
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
    inset: var(--mdc-toolbar-h) 0 var(--mdc-statusbar-h);
    opacity: 0;
    pointer-events: none;
  }
  .force-canvas-wrap {
    flex: 1 1 auto;
    min-width: 0;
    border: 1px solid var(--mdc-border);
    border-radius: var(--mdc-radius-lg);
    overflow: hidden;
    position: relative;
    background: var(--mdc-panel);
    box-shadow: var(--mdc-shadow-lg);
  }
  .force-editor-wrap {
    flex: 0 0 clamp(340px, 34%, 560px);
    width: clamp(340px, 34%, 560px);
    min-width: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .full-error {
    flex: 1;
    display: grid;
    place-items: center;
    color: var(--mdc-error);
    padding: 2rem;
    font-family: var(--mdc-mono);
    font-size: var(--mdc-text-sm);
  }

  /* Shed the widest elements first: search label, then the view-switch labels,
     then the wordmark. Icons and the primary action survive longest. */
  @media (max-width: 1180px) {
    .search-tool {
      width: 240px;
    }
    .key-hints {
      display: none;
    }
  }

  @media (max-width: 1000px) {
    .brand-copy {
      display: none;
    }
    .view-switch button span {
      display: none;
    }
    .view-switch button {
      width: 30px;
      padding: 0;
      justify-content: center;
    }
    .search-label,
    .tool-kbd {
      display: none;
    }
    .search-tool {
      width: 32px;
      padding: 0;
      justify-content: center;
    }
  }

  @media (max-width: 820px) {
    .tool.primary span {
      display: none;
    }
    .tool.primary {
      width: 32px;
      padding: 0;
    }
    .status {
      max-width: 24ch;
    }
  }

  @media (max-width: 700px) {
    .toolbar {
      gap: 0.5rem;
      overflow-x: auto;
      overflow-y: hidden;
      scrollbar-width: none;
    }
    .toolbar::-webkit-scrollbar {
      display: none;
    }
    .toolbar > * {
      flex-shrink: 0;
    }
    .bar-center {
      flex: 0 0 auto;
    }
    .layout,
    .force-layout {
      padding: 0.5rem;
    }
    .layout > :global(.column) {
      display: none;
    }
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
    .statusbar {
      gap: 0.5rem;
    }
  }

  /* View Transitions: the new snapshot fades over the stable old snapshot. */
  :global(html[data-vt-scope="force-editor"]) {
    view-transition-name: none;
  }
  :global(html[data-vt-scope="force-editor"] .force-editor-wrap) {
    view-transition-name: force-editor;
    background: var(--mdc-bg);
    border-radius: var(--mdc-radius-lg);
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
