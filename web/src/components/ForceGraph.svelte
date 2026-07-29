<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    forceSimulation,
    forceManyBody,
    forceLink,
    forceCenter,
    forceCollide,
    forceX,
    forceY,
    type Simulation,
    type SimulationNodeDatum,
  } from "d3-force";
  import type { NodeInfo } from "../lib/types";
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

  interface SimNode extends SimulationNodeDatum {
    id: string;
    title: string;
    depth: number;
    broken: boolean;
    isRoot: boolean;
    isLeaf: boolean;
  }
  interface SimLink {
    source: string | SimNode;
    target: string | SimNode;
  }

  let canvasEl = $state<HTMLCanvasElement | null>(null);
  let containerEl = $state<HTMLDivElement | null>(null);
  let sim: Simulation<SimNode, SimLink> | null = null;
  let nodes: SimNode[] = [];
  let links: SimLink[] = [];
  let rafId = 0;
  let running = true;
  let loadError: string | null = $state(null);
  let hoveredNode: SimNode | null = null;

  // Pan / zoom. viewX/viewY are the screen position of world origin (0,0).
  // This is independent of canvas size, so resizing the canvas never moves
  // the graph — it just gives more/less visible area around the same origin.
  let viewX = 0;
  let viewY = 0;
  let viewK = 1;

  const NODE_COLOR = "#7aa2f7";
  const ROOT_COLOR = "#e0af68";
  const LEAF_COLOR = "#2ac3de";

  let inDegreeMap = new Map<string, number>();
  let outDegreeMap = new Map<string, number>();

  function nodeRadius(n: SimNode): number {
    const inDegree = inDegreeMap.get(n.id) ?? 0;
    const outDegree = outDegreeMap.get(n.id) ?? 0;
    const ior = Math.log1p(inDegree) - Math.log1p(outDegree);
    const r = 6 * (Math.max(0, ior) + 1);
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

  async function loadGraph(): Promise<boolean> {
    const request = ++graphRequest;
    try {
      loadError = null;
      const data = await api.full();
      if (request !== graphRequest) return false;
      nodes = data.nodes.map((n: NodeInfo) => ({
        id: n.fnode,
        title: n.title,
        depth: n.depth,
        broken: n.broken,
        isRoot: false,
        isLeaf: false,
      }));
      const idSet = new Set(nodes.map((n) => n.id));
      links = data.edges
        .filter((e) => idSet.has(e.source) && idSet.has(e.target))
        .map((e) => ({ source: e.source, target: e.target }));
      computeMetadata(nodes, links);
      buildSimulation();
      return true;
    } catch (e) {
      if (request !== graphRequest) return false;
      loadError = e instanceof Error ? e.message : String(e);
      return false;
    }
  }

  // Incremental reload: fetch fresh data but preserve existing node positions
  // so the graph doesn't jump. New nodes appear near the centroid; removed
  // nodes are dropped. Simulation restarts gently with alpha(0.5).
  async function reloadGraph(): Promise<boolean> {
    const request = ++graphRequest;
    try {
      loadError = null;
      const data = await api.full();
      if (request !== graphRequest) return false;
      loadError = null;
      // Preserve positions of existing nodes.
      const posMap = new Map<string, { x: number; y: number }>();
      for (const n of nodes) {
        if (n.x != null && n.y != null) posMap.set(n.id, { x: n.x, y: n.y });
      }
      // Compute centroid for new nodes.
      let cx = 0, cy = 0, count = 0;
      for (const p of posMap.values()) { cx += p.x; cy += p.y; count++; }
      if (count > 0) { cx /= count; cy /= count; }

      const newNodes: SimNode[] = data.nodes.map((n: NodeInfo) => {
        const existing = posMap.get(n.fnode);
        return {
          id: n.fnode,
          title: n.title,
          depth: n.depth,
          broken: n.broken,
          isRoot: false,
          isLeaf: false,
          x: existing?.x ?? cx + (Math.random() - 0.5) * 60,
          y: existing?.y ?? cy + (Math.random() - 0.5) * 60,
          vx: 0,
          vy: 0,
        };
      });
      const idSet = new Set(newNodes.map((n) => n.id));
      const newLinks = data.edges
        .filter((e) => idSet.has(e.source) && idSet.has(e.target))
        .map((e) => ({ source: e.source, target: e.target }));

      computeMetadata(newNodes, newLinks);

      // Hot-swap nodes + links into the running simulation.
      nodes = newNodes;
      links = newLinks;
      if (sim) {
        sim.nodes(nodes);
        const linkForce = sim.force("link") as any;
        if (linkForce) linkForce.links(links);
        sim.alpha(0.5).stop();
        startRaf();
      } else {
        buildSimulation();
      }
      requestRender();
      return true;
    } catch (e) {
      if (request === graphRequest) {
        loadError = e instanceof Error ? e.message : String(e);
      }
      return false;
    }
  }

  // ── Simulation (org-roam-ui model) ──────────────────────────────────────────

  function buildSimulation() {
    // org-roam-ui force model: strong charge + equal per-axis gravity
    // toward origin → isotropic equilibrium → circular disc layout.
    sim = forceSimulation<SimNode>(nodes)
      .force("charge", forceManyBody().strength(-700))
      .force(
        "link",
        forceLink<SimNode, SimLink>(links)
          .id((d) => d.id)
          .strength(0.3)
          .distance(30),
      )
      .force("center", forceCenter(0, 0).strength(0.2))
      .force("collide", forceCollide<SimNode>().radius(20))
      .force("x", forceX(0).strength(0.3))
      .force("y", forceY(0).strength(0.3))
      .alpha(1)
      .alphaDecay(0.05)
      .velocityDecay(0.25)
      .stop();

    for (let i = 0; i < 300; i++) sim.tick();
    requestRender();
  }

  // Render-on-demand flag. Set by requestRender(), consumed by the RAF loop.
  let needsRender = false;
  function requestRender() {
    needsRender = true;
    startRaf();
  }

  function startRaf() {
    if (rafId || !running || !active) return;
    const loop = () => {
      rafId = 0;
      if (!running || !active) return;
      const simulationActive = sim && sim.alpha() > sim.alphaMin();
      if (simulationActive) sim!.tick();
      if (simulationActive || needsRender) {
        render();
        needsRender = false;
      }
      if ((sim && sim.alpha() > sim.alphaMin()) || needsRender) {
        rafId = requestAnimationFrame(loop);
      }
    };
    rafId = requestAnimationFrame(loop);
  }

  function stopRaf() {
    if (rafId) cancelAnimationFrame(rafId);
    rafId = 0;
  }

  // ── Rendering ───────────────────────────────────────────────────────────────

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

    // LOD thresholds.
    const showLabels = viewK > 0.6;
    const showShortFnode = viewK > 0.9;
    const labelAlpha = viewK > 0.9 ? 1 : Math.max(0, (viewK - 0.5) / 0.4);

    // Edges — dim by default; highlight only the selected node's edges.
    for (const link of links) {
      const s = typeof link.source === "string" ? undefined : link.source;
      const t = typeof link.target === "string" ? undefined : link.target;
      if (!s || !t || s.x == null || s.y == null || t.x == null || t.y == null) continue;
      // Highlight only edges connected to the selected node.
      // Outgoing (selected → target) = green; Incoming (source → selected) = purple.
      let stroke: string;
      let lw: number;
      if (selectedFnode && s.id === selectedFnode) {
        stroke = "rgba(158, 206, 106, 0.85)"; // green = outgoing (downstream)
        lw = 2 / viewK;
      } else if (selectedFnode && t.id === selectedFnode) {
        stroke = "rgba(187, 154, 247, 0.85)"; // purple = incoming (upstream)
        lw = 2 / viewK;
      } else if (selectedFnode) {
        // Other edges: dim further when a node is selected.
        stroke = "rgba(86, 95, 137, 0.12)";
        lw = 1 / viewK;
      } else {
        stroke = "rgba(86, 95, 137, 0.3)";
        lw = 1 / viewK;
      }
      ctx.strokeStyle = stroke;
      ctx.lineWidth = lw;
      ctx.beginPath();
      ctx.moveTo(s.x, s.y);
      ctx.lineTo(t.x, t.y);
      ctx.stroke();
    }

    // Nodes.
    for (const n of nodes) {
      if (n.x == null || n.y == null) continue;
      const r = nodeRadius(n);
      const isSelected = selectedFnode === n.id;
      const isHovered = hoveredNode?.id === n.id;
      ctx.beginPath();
      ctx.arc(n.x, n.y, r, 0, 2 * Math.PI);
      ctx.fillStyle = n.broken
        ? "#f7768e"
        : n.isRoot
          ? ROOT_COLOR
          : n.isLeaf
            ? LEAF_COLOR
            : NODE_COLOR;
      ctx.fill();
      if (isSelected) {
        ctx.strokeStyle = "#c0caf5";
        ctx.lineWidth = 2.5 / viewK;
        ctx.stroke();
      } else if (isHovered) {
        ctx.strokeStyle = "#c0caf5";
        ctx.lineWidth = 1.5 / viewK;
        ctx.stroke();
      }

      if (!showLabels) continue;

      if (showShortFnode) {
        ctx.font = `${10 / viewK}px monospace`;
        ctx.fillStyle = `rgba(86, 95, 137, ${labelAlpha * 0.9})`;
        ctx.textAlign = "center";
        ctx.fillText(shortFnode(n.id), n.x, n.y + r + 10 / viewK);
      }
      ctx.font = `${11 / viewK}px sans-serif`;
      ctx.fillStyle = isSelected
        ? "#c0caf5"
        : `rgba(192, 202, 245, ${labelAlpha * 0.8})`;
      ctx.textAlign = "left";
      const label = truncate(n.title, 20);
      ctx.fillText(label, n.x + r + 3 / viewK, n.y + 3 / viewK);
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
    const rect = container.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    canvas.width = rect.width * dpr;
    canvas.height = rect.height * dpr;
    canvas.style.width = `${rect.width}px`;
    canvas.style.height = `${rect.height}px`;
    // Render synchronously — setting canvas.width clears it to transparent,
    // so we must redraw before the browser paints. Using requestRender()
    // (deferred to next RAF) would leave a blank frame.
    render();
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
    // Search in reverse draw order so visually topmost nodes are hit first.
    for (let i = nodes.length - 1; i >= 0; i--) {
      const n = nodes[i]!;
      if (n.x == null || n.y == null) continue;
      const dx = n.x - wx;
      const dy = n.y - wy;
      const r = nodeRadius(n) + 4;
      if (dx * dx + dy * dy <= r * r) return n;
    }
    return null;
  }

  type MouseMode = "idle" | "pan" | "drag-node";
  let mouseMode: MouseMode = "idle";
  let dragNode: SimNode | null = null;
  let mouseStart: { x: number; y: number } | null = null;
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
    mouseStart = { x, y };
    mouseMoved = false;

    const node = findNodeAt(x, y);
    if (node) {
      mouseMode = "drag-node";
      dragNode = node;
      node.fx = node.x;
      node.fy = node.y;
      sim?.alphaTarget(0.3);
      if (sim && sim.alpha() < 0.3) sim.alpha(0.3);
      startRaf();
    } else {
      mouseMode = "pan";
      panStart = { x, y, viewX, viewY };
      canvas.style.cursor = "grabbing";
    }
  }

  function onPointerMove(e: PointerEvent) {
    const canvas = canvasEl;
    if (!canvas) return;
    if (mouseMode !== "idle" && e.pointerId !== activePointerId) return;
    const rect = canvas.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;

    if (mouseMode === "drag-node" && dragNode && mouseStart) {
      mouseMoved = Math.hypot(x - mouseStart.x, y - mouseStart.y) > 3;
      const { x: wx, y: wy } = screenToWorld(x, y);
      dragNode.fx = wx;
      dragNode.fy = wy;
      sim?.alphaTarget(0.3);
      if (sim && sim.alpha() < 0.3) sim.alpha(0.3);
      startRaf();
    } else if (mouseMode === "pan" && panStart) {
      mouseMoved = Math.hypot(x - panStart.x, y - panStart.y) > 3;
      viewX = panStart.viewX + (x - panStart.x);
      viewY = panStart.viewY + (y - panStart.y);
      requestRender();
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

    if (mouseMode === "drag-node" && dragNode) {
      dragNode.fx = null;
      dragNode.fy = null;
      sim?.alphaTarget(0);
      if (!cancelled && !mouseMoved) {
        onSelect(dragNode.id === selectedFnode ? null : dragNode.id);
      }
      dragNode = null;
      startRaf();
    } else if (!cancelled && mouseMode === "pan" && !mouseMoved) {
      const node = findNodeAt(x, y);
      if (!node && selectedFnode) {
        onSelect(null);
      }
    }
    mouseMode = "idle";
    mouseStart = null;
    panStart = null;
    mouseMoved = false;
    canvas.style.cursor = "grab";
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
    viewK = Math.max(0.1, Math.min(5, viewK * factor));

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
    viewK = Math.max(0.1, Math.min(5, Math.min(scaleX, scaleY) * (1 - margin)));
    viewX = cw / 2 - cx * viewK;
    viewY = ch / 2 - cy * viewK;
    render();
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
        // If the graph was loaded while the canvas was hidden (display:none),
        // fit now that we have real dimensions.
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
    sim?.stop();
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
      sim?.stop();
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

<div class="force-container" bind:this={containerEl}>
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
  <button class="ctrl-btn reset-btn" onclick={() => fitToNodes()} title="reset view">⤢</button>
</div>

<style>
  .force-container {
    position: relative;
    width: 100%;
    height: 100%;
    background: var(--mdc-bg);
    overflow: hidden;
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
    cursor: grab;
    touch-action: none;
  }
  .error {
    position: absolute;
    top: 1rem;
    left: 1rem;
    color: var(--mdc-error);
    font-family: var(--mdc-mono);
    font-size: 0.85rem;
    background: var(--mdc-panel);
    padding: 0.5rem 0.8rem;
    border-radius: 4px;
    border: 1px solid var(--mdc-error);
  }
  .ctrl-btn {
    position: absolute;
    bottom: 1rem;
    background: var(--mdc-card);
    color: var(--mdc-fg);
    border: 1px solid var(--mdc-border);
    border-radius: 4px;
    width: 2rem;
    height: 2rem;
    font-size: 1rem;
    cursor: pointer;
    font-family: inherit;
    z-index: 5;
  }
  .ctrl-btn:hover {
    background: var(--mdc-card-hover);
    border-color: var(--mdc-accent);
  }
  .reset-btn {
    right: 1rem;
  }
</style>
