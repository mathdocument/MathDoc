<script lang="ts">
  import { navigate, appState, goBack, goForward, canGoBack, canGoForward, refreshFocused } from "./lib/state.svelte";
  import { api } from "./lib/api";
  import NodeColumn from "./components/NodeColumn.svelte";
  import EditorPane from "./components/EditorPane.svelte";
  import SearchOverlay from "./components/SearchOverlay.svelte";
  import AddDepOverlay from "./components/AddDepOverlay.svelte";
  import RmDepOverlay from "./components/RmDepOverlay.svelte";
  import NewNodeOverlay from "./components/NewNodeOverlay.svelte";
  import ForceGraph from "./components/ForceGraph.svelte";
  import type { NodeDetail } from "./lib/types";

  let showSearch = $state(false);
  let showAddDep = $state(false);
  let showRmDep = $state(false);
  let showNewNode = $state(false);
  let initialLoad = $state(true);
  let initialError = $state<string | null>(null);

  // Top-level view state: three-column layout vs. full-screen force graph.
  let view = $state<"columns" | "force">("columns");
  // Selected fnode in the force graph (drives the side editor panel).
  let forceSelectedFnode = $state<string | null>(null);
  // Increment after dep mutations to trigger ForceGraph data refresh.
  let graphRevision = $state(0);
  // NodeDetail for the force-graph side panel (fetched on selection).
  let forceNodeLoad = $state<
    | { kind: "idle" }
    | { kind: "loading" }
    | { kind: "ready"; node: NodeDetail }
    | { kind: "error"; message: string }
  >({ kind: "idle" });

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
          await navigate(resolved.fnode);
          return;
        }
        const deepest = [...roots].sort((a, b) => b.topo_depth - a.topo_depth)[0]!;
        await navigate(deepest.fnode);
      } catch (e) {
        initialError = e instanceof Error ? e.message : String(e);
      }
    })();
  });

  async function refreshCurrent() {
    if (appState.load.kind !== "ready") return;
    const fnode = appState.load.node.fnode;
    await navigate(fnode, { pushHistory: false, skipTransition: true });
  }

  function refreshForceNode(node: NodeDetail) {
    forceNodeLoad = { kind: "ready", node };
  }

  async function refreshForceNodeRaw() {
    if (!forceSelectedFnode) return;
    try {
      const node = await api.node(forceSelectedFnode);
      forceNodeLoad = { kind: "ready", node };
    } catch {
      // ignore
    }
  }

  function afterDepMutation() {
    graphRevision++;
    if (view === "force") void refreshForceNodeRaw();
    else void refreshCurrent();
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

  async function onForceSelect(fnode: string | null) {
    forceSelectedFnode = fnode;
    if (!fnode) {
      forceNodeLoad = { kind: "idle" };
      return;
    }
    forceNodeLoad = { kind: "loading" };
    try {
      const node = await api.node(fnode);
      forceNodeLoad = { kind: "ready", node };
    } catch (e) {
      forceNodeLoad = {
        kind: "error",
        message: e instanceof Error ? e.message : String(e),
      };
    }
  }

  function toggleGraphView() {
    if (view === "columns") {
      // Enter graph view: select the current column-view node.
      const currentFnode = appState.load.kind === "ready" ? appState.load.node.fnode : null;
      forceSelectedFnode = currentFnode;
      if (currentFnode) {
        void onForceSelect(currentFnode);
      } else {
        forceNodeLoad = { kind: "idle" };
      }
      view = "force";
    } else {
      // Exit graph view: navigate to the selected node in columns.
      // Skip the view transition to avoid a flash — the CSS display swap
      // (force-layout → columns) is instant, and the cross-fade animation
      // would conflict with it.
      if (forceSelectedFnode) {
        void navigate(forceSelectedFnode, { skipTransition: true });
      }
      view = "columns";
      forceSelectedFnode = null;
      forceNodeLoad = { kind: "idle" };
    }
  }
</script>

<div class="app">
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
    <button class="tool primary" onclick={() => (showSearch = true)} title="search">
      search
    </button>
    <button
      class="tool"
      onclick={() => (showAddDep = true)}
      disabled={!activeReady}
      title="add dependency"
    >+ dep</button>
    <button
      class="tool"
      onclick={() => (showRmDep = true)}
      disabled={!activeReady || activeDepens.length === 0}
      title="remove dependency"
    >− dep</button>
    <button
      class="tool"
      onclick={() => (showNewNode = true)}
      disabled={!activeReady}
      title="create node"
    >+node</button>
    <button
      class="tool"
      onclick={toggleGraphView}
      class:primary={view === "force"}
      title={view === "force" ? "back to columns" : "force-directed graph view"}
    >graph</button>
    <span class="spacer"></span>
    <span class="status">{statusLine()}</span>
  </header>

  <!-- Force graph view: always mounted, hidden via CSS when in columns mode. -->
  <main class="force-layout" class:hidden={view !== "force"}>
    <div class="force-canvas-wrap" class:full={!forceSelectedFnode}>
        <ForceGraph onSelect={onForceSelect} selectedFnode={forceSelectedFnode} revision={graphRevision} />
    </div>
    {#if view === "force" && forceSelectedFnode}
      <div class="force-editor-wrap">
        <EditorPane load={forceNodeLoad} onRefresh={refreshForceNode} />
      </div>
    {/if}
  </main>

  <!-- Column view: hidden when in force mode. -->
  <main class="layout" class:hidden={view === "force"}>
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
      <EditorPane load={appState.load} onRefresh={refreshFocused} />
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
</div>

{#if showSearch}
  <SearchOverlay
    onPick={(fnode) => {
      showSearch = false;
      void navigate(fnode, { direction: "neutral" });
    }}
    onClose={() => (showSearch = false)}
  />
{/if}

{#if showAddDep && activeFnode}
  <AddDepOverlay
    fnode={activeFnode}
    existingDepFnodes={activeDepens}
    onAdded={() => afterDepMutation()}
    onClose={() => (showAddDep = false)}
  />
{/if}

{#if showRmDep && activeFnode}
  <RmDepOverlay
    fnode={activeFnode}
    onRemoved={() => afterDepMutation()}
    onClose={() => (showRmDep = false)}
  />
{/if}

{#if showNewNode && activeReady}
  <NewNodeOverlay
    onCreated={(fnode) => { graphRevision++; void navigate(fnode); }}
    onClose={() => (showNewNode = false)}
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
  .layout.hidden {
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
