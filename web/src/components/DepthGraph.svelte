<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { Maximize2 } from "@lucide/svelte";
  import type { GraphFull, NodeInfo } from "../lib/types";
  import { api } from "../lib/api";
  import { shortFnode } from "../lib/format";

  interface Props {
    active: boolean;
    onSelect: (fnode: string | null) => void;
    selectedFnode: string | null;
    /** Increment to trigger a data refresh (after dep mutations). */
    revision?: number;
  }
  let { active, onSelect, selectedFnode, revision = 0 }: Props = $props();

  interface SimNode {
    id: string;
    title: string;
    depth: number;
    broken: boolean;
    isRoot: boolean;
    isLeaf: boolean;
    x?: number;
    y?: number;
  }
  interface SimLink {
    source: string | SimNode;
    target: string | SimNode;
  }

  let canvasEl = $state<HTMLCanvasElement | null>(null);
  let containerEl = $state<HTMLDivElement | null>(null);
  let nodes: SimNode[] = [];
  let links: SimLink[] = [];
  let rafId = 0;
  let running = true;
  let loadError: string | null = $state(null);
  let graphLoading = $state(false);
  let hoveredNode: SimNode | null = null;

  // Pan / zoom. viewX/viewY are the screen position of world origin (0,0).
  // This is independent of canvas size, so resizing the canvas never moves
  // the graph — it just gives more/less visible area around the same origin.
  let viewX = 0;
  let viewY = 0;
  let viewK = 1;

  const NODE_COLOR = "#7c9cff";
  const ROOT_COLOR = "#e8b86d";
  const LEAF_COLOR = "#63d8b2";
  const MAX_NODE_RADIUS = 24;
  const SPATIAL_CELL_SIZE = 64;
  const MIN_ZOOM = 0.0001;
  const MAX_ZOOM = 5;

  let inDegreeMap = new Map<string, number>();
  let outDegreeMap = new Map<string, number>();

  function baseNodeRadius(n: SimNode): number {
    const inDegree = inDegreeMap.get(n.id) ?? 0;
    const outDegree = outDegreeMap.get(n.id) ?? 0;
    const ior = Math.log1p(inDegree) - Math.log1p(outDegree);
    return Math.min(MAX_NODE_RADIUS, 6 * (Math.max(0, ior) + 1));
  }

  function nodeRadius(n: SimNode): number {
    const r = baseNodeRadius(n);
    if (selectedFnode && n.id === selectedFnode) return r + 3;
    if (hoveredNode && n.id === hoveredNode.id) return r + 2;
    return r;
  }

  // Compute directed degrees, root, and leaf status for all rendered nodes.
  function computeMetadata(nodeList: SimNode[], linkList: SimLink[]) {
    inDegreeMap = new Map<string, number>();
    outDegreeMap = new Map<string, number>();
    for (const n of nodeList) {
      inDegreeMap.set(n.id, 0);
      outDegreeMap.set(n.id, 0);
    }
    for (const l of linkList) {
      const s = typeof l.source === "string" ? l.source : l.source.id;
      const t = typeof l.target === "string" ? l.target : l.target.id;
      outDegreeMap.set(s, (outDegreeMap.get(s) ?? 0) + 1);
      inDegreeMap.set(t, (inDegreeMap.get(t) ?? 0) + 1);
    }
    for (const n of nodeList) {
      n.isRoot = (inDegreeMap.get(n.id) ?? 0) === 0;
      n.isLeaf = (outDegreeMap.get(n.id) ?? 0) === 0;
    }
  }

  // ── Data ────────────────────────────────────────────────────────────────────
  let graphRequest = 0;

  function installGraph(data: GraphFull) {
    nodes = data.nodes.map((node: NodeInfo) => ({
      id: node.fnode,
      title: node.title,
      depth: node.depth,
      broken: node.broken,
      isRoot: false,
      isLeaf: false,
    }));
    links = data.edges.map((edge) => ({ source: edge.source, target: edge.target }));
    computeMetadata(nodes, links);
    applyStaticGraphLayout();
  }

  async function loadGraph(): Promise<boolean> {
    const request = ++graphRequest;
    graphLoading = true;
    try {
      loadError = null;
      const data = await api.full();
      if (request !== graphRequest) return false;
      installGraph(data);
      return true;
    } catch (e) {
      if (request !== graphRequest) return false;
      loadError = e instanceof Error ? e.message : String(e);
      return false;
    } finally {
      if (request === graphRequest) graphLoading = false;
    }
  }

  // Reload into the deterministic depth layout after graph mutations.
  async function reloadGraph(): Promise<boolean> {
    const request = ++graphRequest;
    try {
      loadError = null;
      const data = await api.full();
      if (request !== graphRequest) return false;
      loadError = null;
      installGraph(data);
      requestRender();
      return true;
    } catch (e) {
      if (request === graphRequest) {
        loadError = e instanceof Error ? e.message : String(e);
      }
      return false;
    }
  }

  // ── Static depth layout ──────────────────────────────────────────────────────

  function resolveLinkNodes() {
    const nodesById = new Map(nodes.map((node) => [node.id, node]));
    for (const link of links) {
      if (typeof link.source === "string") link.source = nodesById.get(link.source)!;
      if (typeof link.target === "string") link.target = nodesById.get(link.target)!;
    }
  }

  function applyStaticGraphLayout() {
    resolveLinkNodes();

    const layers = new Map<number, SimNode[]>();
    for (const node of nodes) {
      const layer = layers.get(node.depth) ?? [];
      layer.push(node);
      layers.set(node.depth, layer);
    }

    const depths = [...layers.keys()].sort((a, b) => b - a);
    const rowsPerColumn = Math.max(24, Math.ceil(Math.sqrt(nodes.length) * 1.5));
    const spacing = 58;
    let cursorX = 0;
    for (const depth of depths) {
      const layer = layers.get(depth)!;
      layer.sort((a, b) => a.title.localeCompare(b.title) || a.id.localeCompare(b.id));
      const columns = Math.ceil(layer.length / rowsPerColumn);
      const rows = Math.min(rowsPerColumn, layer.length);
      for (let index = 0; index < layer.length; index++) {
        const node = layer[index]!;
        const column = Math.floor(index / rowsPerColumn);
        const row = index % rowsPerColumn;
        node.x = cursorX + column * spacing;
        node.y = (row - (rows - 1) / 2) * spacing;
      }
      cursorX += Math.max(140, columns * spacing + 90);
    }

    const offsetX = cursorX / 2;
    for (const node of nodes) node.x = (node.x ?? 0) - offsetX;
  }

  // Render-on-demand flag. Set by requestRender(), consumed by the RAF loop.
  let needsRender = false;
  function requestRender() {
    needsRender = true;
    startRaf();
  }

  function startRaf() {
    if (rafId || !running || !active) return;
    rafId = requestAnimationFrame(() => {
      rafId = 0;
      if (!running || !active) return;
      if (needsRender) {
        render();
        needsRender = false;
      }
    });
  }

  function stopRaf() {
    if (rafId) cancelAnimationFrame(rafId);
    rafId = 0;
  }

  // ── Rendering ───────────────────────────────────────────────────────────────

  let spatialIndex = new Map<string, SimNode[]>();

  function spatialKey(x: number, y: number): string {
    return `${Math.floor(x / SPATIAL_CELL_SIZE)}:${Math.floor(y / SPATIAL_CELL_SIZE)}`;
  }

  function indexVisibleNode(node: SimNode) {
    const key = spatialKey(node.x!, node.y!);
    const bucket = spatialIndex.get(key);
    if (bucket) bucket.push(node);
    else spatialIndex.set(key, [node]);
  }

  function render() {
    const canvas = canvasEl;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const dpr = window.devicePixelRatio || 1;
    const w = canvas.width / dpr;
    const h = canvas.height / dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    // viewX/viewY = screen position of world origin. Not relative to canvas
    // center, so resizing the canvas doesn't move the graph.
    ctx.translate(viewX, viewY);
    ctx.scale(viewK, viewK);

    const margin = 80 / viewK;
    const minX = -viewX / viewK - margin;
    const maxX = (w - viewX) / viewK + margin;
    const minY = -viewY / viewK - margin;
    const maxY = (h - viewY) / viewK + margin;

    // LOD thresholds.
    const showLabels = viewK > 0.6;
    const showShortFnode = viewK > 0.9;
    const labelAlpha = viewK > 0.9 ? 1 : Math.max(0, (viewK - 0.5) / 0.4);

    // Build a small number of paths instead of issuing one canvas stroke per
    // edge. Cull links wholly outside the viewport and reduce opacity around
    // high-degree selections so dense hubs remain legible.
    const basePath = new Path2D();
    const outgoingPath = new Path2D();
    const incomingPath = new Path2D();
    for (const link of links) {
      const s = typeof link.source === "string" ? undefined : link.source;
      const t = typeof link.target === "string" ? undefined : link.target;
      if (!s || !t || s.x == null || s.y == null || t.x == null || t.y == null) continue;
      if ((s.x < minX && t.x < minX) || (s.x > maxX && t.x > maxX) ||
        (s.y < minY && t.y < minY) || (s.y > maxY && t.y > maxY)) continue;

      let path = basePath;
      if (selectedFnode && s.id === selectedFnode) {
        path = outgoingPath;
      } else if (selectedFnode && t.id === selectedFnode) {
        path = incomingPath;
      }
      path.moveTo(s.x, s.y);
      path.lineTo(t.x, t.y);
    }

    const baseAlpha = Math.max(0.035, 0.28 * Math.min(1, Math.sqrt(5_000 / Math.max(1, links.length))));
    ctx.strokeStyle = `rgba(102, 115, 134, ${selectedFnode ? baseAlpha * 0.35 : baseAlpha})`;
    ctx.lineWidth = 1 / viewK;
    ctx.stroke(basePath);

    if (selectedFnode) {
      const selectedDegree = (inDegreeMap.get(selectedFnode) ?? 0) +
        (outDegreeMap.get(selectedFnode) ?? 0);
      const highlightAlpha = Math.max(0.16, Math.min(0.86, 12 / Math.sqrt(Math.max(1, selectedDegree))));
      ctx.lineWidth = (selectedDegree > 1_000 ? 1.25 : 2) / viewK;
      ctx.strokeStyle = `rgba(99, 216, 178, ${highlightAlpha})`;
      ctx.stroke(outgoingPath);
      ctx.strokeStyle = `rgba(182, 156, 255, ${highlightAlpha})`;
      ctx.stroke(incomingPath);
    }

    // Keep only visible nodes for drawing, labels, and pointer hit testing.
    const visibleNodes: SimNode[] = [];
    spatialIndex = new Map();
    for (const n of nodes) {
      if (n.x == null || n.y == null) continue;
      const r = nodeRadius(n);
      if (n.x + r < minX || n.x - r > maxX || n.y + r < minY || n.y - r > maxY) continue;
      visibleNodes.push(n);
      indexVisibleNode(n);
    }

    const labelStride = Math.max(1, Math.ceil(visibleNodes.length / 500));
    for (let index = 0; index < visibleNodes.length; index++) {
      const n = visibleNodes[index]!;
      const x = n.x!;
      const y = n.y!;
      const r = nodeRadius(n);
      const isSelected = selectedFnode === n.id;
      const isHovered = hoveredNode?.id === n.id;
      ctx.beginPath();
      ctx.arc(x, y, r, 0, 2 * Math.PI);
      ctx.fillStyle = n.broken
        ? "#ff7d8f"
        : n.isRoot
          ? ROOT_COLOR
          : n.isLeaf
            ? LEAF_COLOR
            : NODE_COLOR;
      ctx.fill();
      if (isSelected) {
        ctx.strokeStyle = "#e7edf6";
        ctx.lineWidth = 2.5 / viewK;
        ctx.stroke();
      } else if (isHovered) {
        ctx.strokeStyle = "#e7edf6";
        ctx.lineWidth = 1.5 / viewK;
        ctx.stroke();
      }

      const showNodeLabel = isSelected || isHovered || (showLabels && index % labelStride === 0);
      if (!showNodeLabel) continue;

      if (showShortFnode) {
        ctx.font = `${10 / viewK}px monospace`;
        ctx.fillStyle = `rgba(135, 147, 165, ${labelAlpha * 0.9})`;
        ctx.textAlign = "center";
        ctx.fillText(shortFnode(n.id), x, y + r + 10 / viewK);
      }
      ctx.font = `${11 / viewK}px sans-serif`;
      ctx.fillStyle = isSelected || isHovered
        ? "#e7edf6"
        : `rgba(192, 202, 216, ${labelAlpha * 0.82})`;
      ctx.textAlign = "left";
      const label = truncate(n.title, 20);
      ctx.fillText(label, x + r + 3 / viewK, y + 3 / viewK);
    }
  }

  function truncate(s: string, max: number): string {
    return s.length > max ? s.slice(0, max - 1) + "…" : s;
  }

  // ── Canvas sizing ───────────────────────────────────────────────────────────

  function resizeCanvas() {
    const canvas = canvasEl;
    const container = containerEl;
    if (!canvas || !container) return;
    if (!active) {
      canvas.width = 1;
      canvas.height = 1;
      return;
    }
    const rect = container.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    canvas.width = rect.width * dpr;
    canvas.height = rect.height * dpr;
    canvas.style.width = `${rect.width}px`;
    canvas.style.height = `${rect.height}px`;
    // Redraw on the next frame so showing the view is not blocked by an
    // O(nodes + edges) paint. The grid remains visible beneath the canvas.
    requestRender();
  }

  // ── Interaction ─────────────────────────────────────────────────────────────

  function screenToWorld(x: number, y: number): { x: number; y: number } {
    return {
      x: (x - viewX) / viewK,
      y: (y - viewY) / viewK,
    };
  }

  function findNodeAt(canvasX: number, canvasY: number): SimNode | null {
    const { x: wx, y: wy } = screenToWorld(canvasX, canvasY);
    let candidates = nodes;
    if (nodes.length > 500 && spatialIndex.size > 0) {
      candidates = [];
      const cellX = Math.floor(wx / SPATIAL_CELL_SIZE);
      const cellY = Math.floor(wy / SPATIAL_CELL_SIZE);
      for (let dx = -1; dx <= 1; dx++) {
        for (let dy = -1; dy <= 1; dy++) {
          const bucket = spatialIndex.get(`${cellX + dx}:${cellY + dy}`);
          if (bucket) candidates.push(...bucket);
        }
      }
    }
    // Search in reverse draw order so visually topmost nodes are hit first.
    for (let i = candidates.length - 1; i >= 0; i--) {
      const n = candidates[i]!;
      if (n.x == null || n.y == null) continue;
      const dx = n.x - wx;
      const dy = n.y - wy;
      const r = nodeRadius(n) + 4;
      if (dx * dx + dy * dy <= r * r) return n;
    }
    return null;
  }

  type MouseMode = "idle" | "pan";
  let mouseMode: MouseMode = "idle";
  let pressedNode: SimNode | null = null;
  let panStart: { x: number; y: number; viewX: number; viewY: number } | null = null;
  let mouseMoved = false;
  let activePointerId: number | null = null;

  function onPointerDown(e: PointerEvent) {
    const canvas = canvasEl;
    if (!canvas || e.button !== 0) return;
    activePointerId = e.pointerId;
    canvas.setPointerCapture(e.pointerId);
    const rect = canvas.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    mouseMoved = false;

    pressedNode = findNodeAt(x, y);
    mouseMode = "pan";
    panStart = { x, y, viewX, viewY };
  }

  function onPointerMove(e: PointerEvent) {
    const canvas = canvasEl;
    if (!canvas) return;
    if (mouseMode !== "idle" && e.pointerId !== activePointerId) return;
    const rect = canvas.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;

    if (mouseMode === "pan" && panStart) {
      mouseMoved = Math.hypot(x - panStart.x, y - panStart.y) > 3;
      if (mouseMoved) {
        viewX = panStart.viewX + (x - panStart.x);
        viewY = panStart.viewY + (y - panStart.y);
        canvas.style.cursor = "grabbing";
        requestRender();
      }
    } else {
      const node = findNodeAt(x, y);
      const changed = (node?.id ?? null) !== (hoveredNode?.id ?? null);
      hoveredNode = node;
      canvas.style.cursor = node ? "pointer" : "grab";
      if (changed) requestRender();
    }
  }

  function finishPointer(e: PointerEvent | null, cancelled: boolean) {
    const canvas = canvasEl;
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const x = e ? e.clientX - rect.left : 0;
    const y = e ? e.clientY - rect.top : 0;

    if (!cancelled && mouseMode === "pan" && !mouseMoved) {
      if (pressedNode) {
        onSelect(pressedNode.id === selectedFnode ? null : pressedNode.id);
      } else if (selectedFnode) {
        onSelect(null);
      }
    }
    mouseMode = "idle";
    panStart = null;
    pressedNode = null;
    mouseMoved = false;
    canvas.style.cursor = findNodeAt(x, y) ? "pointer" : "grab";
    if (activePointerId !== null && canvas.hasPointerCapture(activePointerId)) {
      canvas.releasePointerCapture(activePointerId);
    }
    activePointerId = null;
  }

  function onPointerUp(e: PointerEvent) {
    if (e.pointerId === activePointerId) finishPointer(e, false);
  }

  function onPointerCancel(e: PointerEvent) {
    if (e.pointerId === activePointerId) finishPointer(e, true);
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const canvas = canvasEl;
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;

    // Mouse position in world coords before zoom.
    const worldBefore = screenToWorld(x, y);
    const factor = Math.exp(-e.deltaY * 0.0015);
    viewK = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, viewK * factor));

    // Adjust viewX/viewY so the world point under the cursor stays fixed.
    viewX = x - worldBefore.x * viewK;
    viewY = y - worldBefore.y * viewK;
    requestRender();
  }

  function fitToNodes() {
    const canvas = canvasEl;
    if (!canvas || nodes.length === 0) return;
    // Compute bounding box of all nodes.
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (const n of nodes) {
      if (n.x == null || n.y == null) continue;
      minX = Math.min(minX, n.x);
      maxX = Math.max(maxX, n.x);
      minY = Math.min(minY, n.y);
      maxY = Math.max(maxY, n.y);
    }
    if (minX === Infinity) return;
    const graphW = maxX - minX;
    const graphH = maxY - minY;
    const cx = (minX + maxX) / 2;
    const cy = (minY + maxY) / 2;
    const cw = canvas.clientWidth;
    const ch = canvas.clientHeight;
    const margin = 0.2;
    const scaleX = cw / (graphW + 2 * 20);
    const scaleY = ch / (graphH + 2 * 20);
    viewK = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, Math.min(scaleX, scaleY) * (1 - margin)));
    viewX = cw / 2 - cx * viewK;
    viewY = ch / 2 - cy * viewK;
    render();
  }

  /** Zoom by `factor` around the canvas center (used by the zoom buttons). */
  function zoomBy(factor: number) {
    const canvas = canvasEl;
    if (!canvas) return;
    const cw = canvas.clientWidth;
    const ch = canvas.clientHeight;
    const anchor = screenToWorld(cw / 2, ch / 2);
    viewK = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, viewK * factor));
    viewX = cw / 2 - anchor.x * viewK;
    viewY = ch / 2 - anchor.y * viewK;
    requestRender();
  }

  // ── Lifecycle ───────────────────────────────────────────────────────────────

  let resizeObserver: ResizeObserver | null = null;
  let needsFit = false;
  let graphInitialized = false;
  let graphDirty = false;
  let graphLoadPromise: Promise<void> | null = null;

  function ensureGraphLoaded(): Promise<void> {
    if (!running) return Promise.resolve();
    if (graphLoadPromise) return graphLoadPromise;
    if (graphInitialized && !graphDirty) return Promise.resolve();

    graphLoadPromise = (async () => {
      if (!graphInitialized) {
        const loaded = await loadGraph();
        if (!loaded) {
          graphDirty = false;
          return;
        }
        if (!running) return;
        graphInitialized = true;
        if (canvasEl && canvasEl.clientWidth > 0) {
          fitToNodes();
        } else {
          needsFit = true;
        }
      }
      if (graphDirty) {
        graphDirty = false;
        if (!await reloadGraph()) return;
      }
      if (!running) return;
      resizeCanvas();
      requestRender();
    })().finally(() => {
      graphLoadPromise = null;
      if (running && active && graphDirty) void ensureGraphLoaded();
    });
    return graphLoadPromise;
  }

  onMount(() => {
    resizeCanvas();
    if (canvasEl) {
      canvasEl.style.cursor = "grab";
      // Wheel must be registered with passive:false so preventDefault works.
      canvasEl.addEventListener("wheel", onWheel, { passive: false });
    }
    if (containerEl) {
      resizeObserver = new ResizeObserver(() => {
        resizeCanvas();
        // Fit after any layout change that gives the canvas usable dimensions.
        if (needsFit && canvasEl && canvasEl.clientWidth > 0) {
          needsFit = false;
          fitToNodes();
        }
      });
      resizeObserver.observe(containerEl);
    }
  });

  onDestroy(() => {
    graphRequest++;
    running = false;
    finishPointer(null, true);
    stopRaf();
    resizeObserver?.disconnect();
    canvasEl?.removeEventListener("wheel", onWheel);
  });

  // Re-render when selection changes from outside.
  $effect(() => {
    void selectedFnode;
    requestRender();
  });

  $effect(() => {
    if (active) {
      void ensureGraphLoaded();
      resizeCanvas();
      requestRender();
    } else {
      finishPointer(null, true);
      stopRaf();
      if (canvasEl) {
        canvasEl.width = 1;
        canvasEl.height = 1;
      }
    }
  });

  // Reload graph data when revision changes (after dep mutations).
  let revisionInitialized = false;
  $effect(() => {
    void revision;
    if (!revisionInitialized) {
      revisionInitialized = true;
      return;
    }
    if (!graphInitialized) {
      if (graphLoadPromise) graphDirty = true;
      else if (active) void ensureGraphLoaded();
      return;
    }
    graphDirty = true;
    if (active) void ensureGraphLoaded();
  });
</script>

<svelte:window onblur={() => finishPointer(null, true)} />

<div class="graph-container" bind:this={containerEl}>
  {#if graphLoading}
    <div class="graph-loading" role="status">
      <span class="spinner"></span>
      <span>Loading graph</span>
    </div>
  {/if}
  {#if loadError}
    <div class="error">{loadError}</div>
  {/if}
  <canvas
    bind:this={canvasEl}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerCancel}
  ></canvas>
  <button class="ctrl-btn reset-btn" onclick={() => fitToNodes()} title="Fit graph to view">
    <Maximize2 size={14} strokeWidth={1.8} />
    <span>Fit graph</span>
  </button>
  <div class="zoom-cluster" role="group" aria-label="zoom controls">
    <button class="ctrl-btn zoom-btn" onclick={() => zoomBy(1.35)} title="Zoom in" aria-label="Zoom in">+</button>
    <button class="ctrl-btn zoom-btn" onclick={() => zoomBy(1 / 1.35)} title="Zoom out" aria-label="Zoom out">−</button>
  </div>
</div>

<style>
  .graph-container {
    position: relative;
    width: 100%;
    height: 100%;
    background-color: var(--mdc-bg);
    background-image:
      linear-gradient(rgba(124, 156, 255, 0.025) 1px, transparent 1px),
      linear-gradient(90deg, rgba(124, 156, 255, 0.025) 1px, transparent 1px);
    background-size: 28px 28px;
    overflow: hidden;
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
    cursor: grab;
    touch-action: none;
  }
  .graph-loading {
    position: absolute;
    inset: 0;
    z-index: 4;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.55rem;
    color: var(--mdc-dim);
    background: rgba(9, 13, 20, 0.3);
    font-family: var(--mdc-mono);
    font-size: 0.68rem;
    pointer-events: none;
  }
  .spinner {
    width: 14px;
    height: 14px;
    border: 2px solid var(--mdc-border-strong);
    border-top-color: var(--mdc-accent);
    border-radius: 50%;
    animation: graph-spin 0.8s linear infinite;
  }
  @keyframes graph-spin {
    to { transform: rotate(360deg); }
  }
  .error {
    position: absolute;
    top: 0.85rem;
    left: 0.85rem;
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.72rem;
    background: rgba(15, 21, 31, 0.94);
    padding: 0.55rem 0.75rem;
    border-radius: var(--mdc-radius-sm);
    border: 1px solid var(--mdc-error);
  }
  .ctrl-btn {
    position: absolute;
    bottom: 0.85rem;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    min-height: 32px;
    background: rgba(20, 28, 40, 0.94);
    color: var(--mdc-fg-soft);
    border: 1px solid var(--mdc-border);
    border-radius: 7px;
    padding: 0 0.65rem;
    font-size: 0.68rem;
    font-weight: 600;
    cursor: pointer;
    font-family: inherit;
    z-index: 5;
  }
  .ctrl-btn:hover {
    background: var(--mdc-card-hover);
    border-color: var(--mdc-accent);
    color: var(--mdc-fg);
  }
  .reset-btn {
    right: 0.85rem;
  }
  .zoom-cluster {
    position: absolute;
    bottom: 0.85rem;
    left: 0.85rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    z-index: 5;
  }
  .zoom-btn {
    position: static;
    width: 32px;
    padding: 0;
    font-size: 1rem;
    font-weight: 700;
    line-height: 1;
  }
</style>
