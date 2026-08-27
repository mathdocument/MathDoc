<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    ArrowLeft,
    ArrowRight,
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
    cancelNavigation,
    refreshFocused,
    browserHistoryEntry,
    browserHistoryTarget,
    commitClearedHistory,
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
  import type { GraphCheckReport, NodeDetail, SrcBlock } from "./lib/types";
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
    | { kind: "new-node" };

  let overlay = $state<Overlay>({ kind: "none" });
  let initialLoad = $state(true);
  let initialError = $state<string | null>(null);
  let refreshError = $state<string | null>(null);
  let refreshing = $state(false);
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
  // Selected fnode in the force graph (drives the side editor panel).
  let forceSelectedFnode = $state<string | null>(null);
  // Increment after dependency mutations to refresh the graph data.
  let graphRevision = $state(0);
  let forceLoadRequest = 0;
  let relationRequest = 0;
  let viewRequest = 0;
  let forceRelationsDirty = false;
  // NodeDetail for the force-graph side panel (fetched on selection).
  let forceNodeLoad = $state<LoadState>({ kind: "idle" });
  let forceEditorRevision = $state(0);
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

  async function refreshGraphCheck(): Promise<boolean> {
    const request = ++graphCheckRequest;
    graphCheckLoading = true;
    graphCheckError = null;
    try {
      const report = await api.graphCheck();
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
      const target = browserHistoryTarget(entry);
      const committed = view === "force"
        ? await onForceSelect(entry.fnode, {
            pushHistory: false,
            historyIndex: entry.index,
            historyEntries: entry.entries,
            browserHistory: "replace",
            preserveOnFailure: true,
          })
        : await navigate(target, {
            pushHistory: false,
            historyIndex: entry.index,
            historyEntries: entry.entries,
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
      graphCheckRequest++;
      resolveColumnsEditorReady?.();
      resolveForceEditorReady?.();
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
        // URL hash can override the default even when a cyclic graph has no roots.
        const hash = window.location.hash.slice(1);
        const params = new URLSearchParams(hash);
        const currentEntry = browserHistoryEntry(window.history.state);
        const ref = params.get("ref") ??
          (currentEntry?.fnode === null ? browserHistoryTarget(currentEntry) : null);
        if (ref) {
          const resolved = await api.resolve(ref);
          await navigate(resolved.fnode, initialHistoryOptions(resolved.fnode));
          return;
        }
        const roots = (await api.roots()).filter((node) => !node.broken);
        if (roots.length === 0) {
          const graph = await api.full();
          const fallback = graph.nodes.find((node) => !node.broken);
          if (!fallback) {
            initialError = "workspace has no valid nodes — create one with New node";
            return;
          }
          await navigate(fallback.fnode, initialHistoryOptions(fallback.fnode));
          return;
        }
        const deepest = [...roots].sort((a, b) => b.topo_depth - a.topo_depth)[0]!;
        await navigate(deepest.fnode, initialHistoryOptions(deepest.fnode));
      } catch (e) {
        initialError = e instanceof Error ? e.message : String(e);
      } finally {
        void refreshGraphCheck();
      }
    })();
  });

  async function refreshCurrent(
    skipUnsavedGuard = false,
    forceDiscovery = true,
  ): Promise<boolean> {
    if (appState.load.kind !== "ready") return true;
    const fnode = appState.load.node.fnode;
    return navigate(fnode, {
      pushHistory: false,
      skipTransition: true,
      skipUnsavedGuard,
      forceDiscovery,
    });
  }

  async function refreshView() {
    if (!confirmDiscardDrafts()) return;
    if (!await settlePendingMutations()) return;
    if (refreshing) return;
    refreshError = null;
    refreshing = true;
    try {
      const checked = await refreshGraphCheck();
      const refreshed = view === "force"
        ? await refreshForceNodeRaw(true, !checked)
        : await refreshCurrent(true, !checked);
      if (refreshed || checked) graphRevision++;
      if (!refreshed) refreshError = "refresh request failed";
    } finally {
      refreshing = false;
    }
  }

  function refreshForceNode(node: NodeDetail, graphChanged = false) {
    if (node.fnode !== forceSelectedFnode) return;
    forceLoadRequest++;
    if (graphChanged) graphRevision++;
    forceNodeLoad = { kind: "ready", node };
  }

  function refreshColumnNode(node: NodeDetail, graphChanged = false) {
    if (graphChanged) graphRevision++;
    refreshFocused(node);
  }

  async function refreshForceNodeRaw(
    skipUnsavedGuard = false,
    forceDiscovery = true,
  ): Promise<boolean> {
    if (!forceSelectedFnode) return true;
    if (!skipUnsavedGuard && !confirmDiscardDrafts()) return false;
    if (!await settlePendingMutations()) return false;
    if (!forceSelectedFnode) return true;
    const confirmedDraftRevision = unsavedDraftRevision();
    const targetFnode = forceSelectedFnode;
    const request = ++forceLoadRequest;
    try {
      const node = await api.node(targetFnode, forceDiscovery);
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

  function sameBlocks(left: SrcBlock[], right: SrcBlock[]): boolean {
    return left.length === right.length && left.every((block, index) => {
      const other = right[index];
      if (!other || block.srctype !== other.srctype || block.content !== other.content) {
        return false;
      }
      const keys = Object.keys(block.metadata).sort();
      const otherKeys = Object.keys(other.metadata).sort();
      return keys.length === otherKeys.length &&
        keys.every((key, keyIndex) =>
          key === otherKeys[keyIndex] && block.metadata[key] === other.metadata[key]);
    });
  }

  function relationUpdate(current: NodeDetail, updated: NodeDetail): NodeDetail {
    const contentUnchanged = current.title === updated.title &&
      sameBlocks(current.blocks, updated.blocks);
    if (!contentUnchanged) {
      refreshError = "dependencies updated, but node content changed externally; refresh before editing";
      return { ...current, depens: updated.depens };
    }
    return {
      ...current,
      revision: updated.revision,
      depens: updated.depens,
      formalization: updated.formalization,
    };
  }

  function afterDepMutation(updated: NodeDetail, delta: { nodes: number; edges: number }) {
    const request = ++relationRequest;
    refreshError = null;
    graphRevision++;
    applyGraphStatsDelta(delta.nodes, delta.edges);
    if (forceNodeLoad.kind === "ready" && forceNodeLoad.node.fnode === updated.fnode) {
      forceNodeLoad = { kind: "ready", node: relationUpdate(forceNodeLoad.node, updated) };
    }
    if (appState.load.kind === "ready" && appState.load.node.fnode === updated.fnode) {
      refreshFocused(relationUpdate(appState.load.node, updated));
    }
    forceRelationsDirty = true;
    if (appState.load.kind !== "ready" || appState.load.node.fnode !== updated.fnode) return;
    void api.nodeView(updated.fnode).then((nodeView) => {
      if (request !== relationRequest) return;
      if (appState.load.kind === "ready" && appState.load.node.fnode === updated.fnode) {
        appState.referrers = { items: nodeView.referrers, selected: -1 };
        appState.children = { items: nodeView.children, selected: -1 };
        forceRelationsDirty = false;
      }
    }).catch((e) => {
      if (request !== relationRequest) return;
      refreshError = e instanceof Error ? e.message : String(e);
    });
  }

  function afterNodeCreated(fnode: string, skipUnsavedGuard = false) {
    initialError = null;
    graphRevision++;
    applyGraphStatsDelta(1, 0);
    if (view === "force") void onForceSelect(fnode, { skipUnsavedGuard });
    else void navigate(fnode, { skipUnsavedGuard });
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
  let activeRevision = $derived(
    view === "force"
      ? forceNodeLoad.kind === "ready" ? forceNodeLoad.node.revision : null
      : appState.load.kind === "ready" ? appState.load.node.revision : null,
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
      historyEntries?: string[];
      browserHistory?: BrowserHistoryMode;
      preserveOnFailure?: boolean;
    } = {},
  ): Promise<boolean> {
    if (!opts.skipUnsavedGuard && !confirmDiscardDrafts()) return false;
    if (!await settlePendingMutations()) return false;
    const confirmedDraftRevision = unsavedDraftRevision();
    const request = ++forceLoadRequest;
    if (!fnode) {
      let committed = false;
      await withViewTransition("neutral", () => {
        if (request !== forceLoadRequest) return;
        if (unsavedDraftRevision() !== confirmedDraftRevision) return;
        forceEditorRevision++;
        forceSelectedFnode = null;
        forceNodeLoad = { kind: "idle" };
        commitClearedHistory({
          pushHistory: opts.pushHistory,
          historyIndex: opts.historyIndex,
          historyEntries: opts.historyEntries,
          browserHistory: opts.browserHistory,
        });
        appState.navigationError = null;
        appState.failedNavigationFnode = null;
        committed = true;
      }, "force-editor");
      return committed;
    }
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
          historyEntries: opts.historyEntries,
          browserHistory: opts.browserHistory,
        });
        appState.navigationError = null;
        appState.failedNavigationFnode = null;
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
    cancelNavigation();
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
          forceSelectedFnode = node.fnode;
          forceNodeLoad = { kind: "ready", node };
          commitFocusedHistory(node.fnode, { pushHistory: false });
        } else {
          forceSelectedFnode = null;
          forceNodeLoad = { kind: "idle" };
        }
        forceRelationsDirty = false;
        if (request !== viewRequest) return;
        const editorReady = new Promise<void>((resolve) => {
          resolveForceEditorReady = resolve;
        });
        forceEditorMounted = true;
        await tick();
        await editorReady;
        if (request !== viewRequest) return;
        view = "force";
        await tick();
        columnsMounted = false;
        await tick();
        if (forceSelectedFnode) {
          void refreshReusedForceNode(
            forceSelectedFnode,
            forceRequest,
            unsavedDraftRevision(),
          );
        }
      } else {
        // Keep the complete graph view visible until the column data is ready.
        forceLoadRequest++;
        const target = forceSelectedFnode ?? appState.history[appState.historyIdx] ?? null;
        const canReuseColumns = target !== null &&
          appState.load.kind === "ready" &&
          appState.load.node.fnode === target &&
          !forceRelationsDirty;
        if (canReuseColumns && forceNodeLoad.kind === "ready") {
          refreshFocused(forceNodeLoad.node);
        } else if (target) {
          const navigated = await navigate(target, {
            pushHistory: false,
            skipTransition: true,
            skipUnsavedGuard: true,
            browserHistory: "replace",
          });
          if (!navigated) return;
        }
        if (request !== viewRequest) return;
        const editorReady = new Promise<void>((resolve) => {
          resolveColumnsEditorReady = resolve;
        });
        columnsMounted = true;
        await tick();
        await editorReady;
        if (request !== viewRequest) return;
        view = "columns";
        await tick();
        forceEditorMounted = false;
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

<div
  class="app"
  inert={overlay.kind !== "none" || viewSwitching || refreshing}
  aria-busy={viewSwitching || refreshing}
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
  {#if appState.navigationError || refreshError}
    <div class="app-error" role="alert">
      <span>{appState.navigationError ?? refreshError}</span>
      <button onclick={() => {
        if (appState.failedNavigationFnode) {
          const target = appState.failedNavigationFnode;
          if (view === "force") void onForceSelect(target);
          else void navigate(target);
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
          onSelect={onForceSelect}
          selectedFnode={forceSelectedFnode}
          revision={graphRevision}
        />
    </div>
    <div class="force-editor-wrap">
      {#if forceEditorMounted}
        {#key forceEditorRevision}
          <EditorPane
            load={forceNodeLoad}
            onRefresh={refreshForceNode}
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
        items={appState.referrers.items}
        selected={appState.referrers.selected}
        accent="up"
        lastVisitedFnode={appState.lastVisitedFnode}
        onSelect={(fnode) => navigate(fnode, { direction: "up" })}
        onHover={(i) => (appState.referrers.selected = i)}
      />
      {#key appState.editorRevision}
        <EditorPane
          load={appState.load}
          onRefresh={refreshColumnNode}
          onReady={markColumnsEditorReady}
        />
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
      targetRevision={activeRevision!}
      onAdded={afterDepMutation}
      onClose={() => (overlay = { kind: "none" })}
    />
  {/key}
{/if}

{#if overlay.kind === "rm-dep"}
  {#key overlay.target}
    <RmDepOverlay
      targetFnode={overlay.target}
      targetRevision={activeRevision!}
      onRemoved={afterDepMutation}
      onClose={() => (overlay = { kind: "none" })}
    />
  {/key}
{/if}

{#if overlay.kind === "new-node"}
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
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.22);
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
  .graph-stats.checking { opacity: 0.65; }
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
  .force-layout.hidden,
  .layout.hidden {
    position: absolute;
    inset: 58px 0 0;
    opacity: 0;
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
