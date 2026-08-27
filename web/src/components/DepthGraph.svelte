<script lang="ts">
  import { onMount, onDestroy, untrack } from "svelte";
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
    label: string;
    depth: number;
    isRoot: boolean;
    isLeaf: boolean;
    baseRadius: number;
    x: number;
    y: number;
  }
  interface SimLink {
    source: SimNode;
    target: SimNode;
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
  const EDGE_BUCKET_SIZE = 512;
  const MAX_EDGE_BUCKET_SPAN = 16;
  const MIN_ZOOM = 0.0001;
  const MAX_ZOOM = 5;

  let inDegreeMap = new Map<string, number>();
  let outDegreeMap = new Map<string, number>();
  let nodeSpatialIndex = new Map<string, SimNode[]>();
  let nodesByX: SimNode[] = [];
  let edgeBuckets = new Map<number, SimLink[]>();
  let wideEdges: SimLink[] = [];
  let minEdgeBucket = 0;
  let maxEdgeBucket = -1;
  let dprQuery: MediaQueryList | null = null;
  let fullEdgeSelection: string | null | undefined;
  let fullBasePath = new Path2D();
  let fullOutgoingPath = new Path2D();
  let fullIncomingPath = new Path2D();
  let graphBounds = { minX: Infinity, maxX: -Infinity, minY: Infinity, maxY: -Infinity };

  function nodeRadius(n: SimNode): number {
    const r = n.baseRadius;
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
      const s = l.source.id;
      const t = l.target.id;
      outDegreeMap.set(s, (outDegreeMap.get(s) ?? 0) + 1);
      inDegreeMap.set(t, (inDegreeMap.get(t) ?? 0) + 1);
    }
    for (const n of nodeList) {
      const inDegree = inDegreeMap.get(n.id) ?? 0;
      const outDegree = outDegreeMap.get(n.id) ?? 0;
      n.isRoot = inDegree === 0;
      n.isLeaf = outDegree === 0;
      const ior = Math.log1p(inDegree) - Math.log1p(outDegree);
      n.baseRadius = Math.min(MAX_NODE_RADIUS, 6 * (Math.max(0, ior) + 1));
    }
  }

  // ── Data ────────────────────────────────────────────────────────────────────
  let graphRequest = 0;

  function installGraph(data: GraphFull) {
    nodes = data.nodes.map((node: NodeInfo) => ({
      id: node.fnode,
      title: node.title,
      label: truncate(node.title, 20),
      depth: node.depth,
      isRoot: false,
      isLeaf: false,
      baseRadius: 6,
      x: 0,
      y: 0,
    }));
    links = data.edges.map(([source, target]) => ({
      source: nodes[source]!,
      target: nodes[target]!,
    }));
    computeMetadata(nodes, links);
    applyStaticGraphLayout();
    computeGraphBounds();
    buildSpatialIndexes();
    fullEdgeSelection = undefined;
    fullBasePath = new Path2D();
    fullOutgoingPath = new Path2D();
    fullIncomingPath = new Path2D();
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

  function lexicalCompare(left: string, right: string): number {
    return left < right ? -1 : left > right ? 1 : 0;
  }

  function applyStaticGraphLayout() {
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
      layer.sort((a, b) => lexicalCompare(a.title, b.title) || lexicalCompare(a.id, b.id));
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

  function computeGraphBounds() {
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (const node of nodes) {
      minX = Math.min(minX, node.x - node.baseRadius);
      maxX = Math.max(maxX, node.x + node.baseRadius);
      minY = Math.min(minY, node.y - node.baseRadius);
      maxY = Math.max(maxY, node.y + node.baseRadius);
    }
    for (const { source, target } of links) {
      if (source !== target) continue;
      const loopRadius = source.baseRadius + 8;
      minX = Math.min(minX, source.x - loopRadius);
      maxX = Math.max(maxX, source.x + loopRadius);
      minY = Math.min(minY, source.y - 2 * loopRadius);
    }
    graphBounds = { minX, maxX, minY, maxY };
  }

  function buildSpatialIndexes() {
    nodeSpatialIndex = new Map();
    nodesByX = [...nodes].sort((a, b) => a.x - b.x);
    for (const node of nodes) {
      const key = spatialKey(node.x, node.y);
      const bucket = nodeSpatialIndex.get(key);
      if (bucket) bucket.push(node);
      else nodeSpatialIndex.set(key, [node]);
    }

    edgeBuckets = new Map();
    wideEdges = [];
    minEdgeBucket = Infinity;
    maxEdgeBucket = -Infinity;
    for (const link of links) {
      const { source, target } = link;
      const loopRadius = source === target ? source.baseRadius + 8 : 0;
      const first = Math.floor((Math.min(source.x, target.x) - loopRadius) / EDGE_BUCKET_SIZE);
      const last = Math.floor((Math.max(source.x, target.x) + loopRadius) / EDGE_BUCKET_SIZE);
      if (last - first > MAX_EDGE_BUCKET_SPAN) {
        wideEdges.push(link);
        continue;
      }
      minEdgeBucket = Math.min(minEdgeBucket, first);
      maxEdgeBucket = Math.max(maxEdgeBucket, last);
      for (let bucketIndex = first; bucketIndex <= last; bucketIndex++) {
        const bucket = edgeBuckets.get(bucketIndex);
        if (bucket) bucket.push(link);
        else edgeBuckets.set(bucketIndex, [link]);
      }
    }
    if (edgeBuckets.size === 0) {
      minEdgeBucket = 0;
      maxEdgeBucket = -1;
    }
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

  function spatialKey(x: number, y: number): string {
    return `${Math.floor(x / SPATIAL_CELL_SIZE)}:${Math.floor(y / SPATIAL_CELL_SIZE)}`;
  }

  function firstNodeAtOrAfter(x: number): number {
    let low = 0;
    let high = nodesByX.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (nodesByX[middle]!.x < x) low = middle + 1;
      else high = middle;
    }
    return low;
  }

  function appendEdge(path: Path2D, link: SimLink) {
    const { source, target } = link;
    if (source === target) {
      const loopRadius = source.baseRadius + 8;
      path.moveTo(source.x, source.y);
      path.arc(source.x, source.y - loopRadius, loopRadius, Math.PI / 2, Math.PI * 2.5);
    } else {
      path.moveTo(source.x, source.y);
      path.lineTo(target.x, target.y);
    }
  }

  function ensureFullEdgePaths(selection: string | null) {
    if (fullEdgeSelection === selection) return;
    fullEdgeSelection = selection;
    fullBasePath = new Path2D();
    fullOutgoingPath = new Path2D();
    fullIncomingPath = new Path2D();
    for (const link of links) {
      const path = selection && link.source.id === selection
        ? fullOutgoingPath
        : selection && link.target.id === selection
          ? fullIncomingPath
          : fullBasePath;
      appendEdge(path, link);
    }
  }

  function edgeOutsideViewport(
    link: SimLink,
    minX: number,
    maxX: number,
    minY: number,
    maxY: number,
  ): boolean {
    const { source, target } = link;
    if (source === target) {
      const loopRadius = source.baseRadius + 8;
      return source.x + loopRadius < minX || source.x - loopRadius > maxX ||
        source.y < minY || source.y - 2 * loopRadius > maxY;
    }
    return (source.x < minX && target.x < minX) ||
      (source.x > maxX && target.x > maxX) ||
      (source.y < minY && target.y < minY) ||
      (source.y > maxY && target.y > maxY);
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
    const graphSelection = selectedFnode && inDegreeMap.has(selectedFnode)
      ? selectedFnode
      : null;

    // Build a small number of paths instead of issuing one canvas stroke per
    // edge. Cull links wholly outside the viewport and reduce opacity around
    // high-degree selections so dense hubs remain legible.
    const firstBucket = Math.max(minEdgeBucket, Math.floor(minX / EDGE_BUCKET_SIZE));
    const lastBucket = Math.min(maxEdgeBucket, Math.floor(maxX / EDGE_BUCKET_SIZE));
    let linkCandidates: Iterable<SimLink> = links;
    if (lastBucket < firstBucket) {
      linkCandidates = wideEdges;
    } else {
      let candidateCost = wideEdges.length;
      for (let bucketIndex = firstBucket; bucketIndex <= lastBucket; bucketIndex++) {
        candidateCost += edgeBuckets.get(bucketIndex)?.length ?? 0;
      }
      if (candidateCost < links.length) {
        const visibleLinks = new Set<SimLink>(wideEdges);
        for (let bucketIndex = firstBucket; bucketIndex <= lastBucket; bucketIndex++) {
          const bucket = edgeBuckets.get(bucketIndex);
          if (bucket) for (const link of bucket) visibleLinks.add(link);
        }
        linkCandidates = visibleLinks;
      }
    }
    let basePath: Path2D;
    let outgoingPath: Path2D;
    let incomingPath: Path2D;
    const fullGraphVisible = minX <= graphBounds.minX && maxX >= graphBounds.maxX &&
      minY <= graphBounds.minY && maxY >= graphBounds.maxY;
    if (linkCandidates === links && fullGraphVisible) {
      ensureFullEdgePaths(graphSelection);
      basePath = fullBasePath;
      outgoingPath = fullOutgoingPath;
      incomingPath = fullIncomingPath;
    } else {
      basePath = new Path2D();
      outgoingPath = new Path2D();
      incomingPath = new Path2D();
      for (const link of linkCandidates) {
        const { source, target } = link;
        if (edgeOutsideViewport(link, minX, maxX, minY, maxY)) continue;
        const path = graphSelection && source.id === graphSelection
          ? outgoingPath
          : graphSelection && target.id === graphSelection
            ? incomingPath
            : basePath;
        appendEdge(path, link);
      }
    }

    const baseAlpha = Math.max(0.035, 0.28 * Math.min(1, Math.sqrt(5_000 / Math.max(1, links.length))));
    ctx.strokeStyle = `rgba(102, 115, 134, ${graphSelection ? baseAlpha * 0.35 : baseAlpha})`;
    ctx.lineWidth = 1 / viewK;
    ctx.stroke(basePath);

    if (graphSelection) {
      const selectedDegree = (inDegreeMap.get(graphSelection) ?? 0) +
        (outDegreeMap.get(graphSelection) ?? 0);
      const highlightAlpha = Math.max(0.16, Math.min(0.86, 12 / Math.sqrt(Math.max(1, selectedDegree))));
      ctx.lineWidth = (selectedDegree > 1_000 ? 1.25 : 2) / viewK;
      ctx.strokeStyle = `rgba(99, 216, 178, ${highlightAlpha})`;
      ctx.stroke(outgoingPath);
      ctx.strokeStyle = `rgba(182, 156, 255, ${highlightAlpha})`;
      ctx.stroke(incomingPath);
    }

    // Keep only visible nodes for drawing, labels, and pointer hit testing.
    const visibleNodes: SimNode[] = [];
    const nodeMargin = MAX_NODE_RADIUS + 3;
    const firstNode = firstNodeAtOrAfter(minX - nodeMargin);
    for (let index = firstNode; index < nodesByX.length; index++) {
      const n = nodesByX[index]!;
      if (n.x > maxX + nodeMargin) break;
      const r = nodeRadius(n);
      if (n.x + r < minX || n.x - r > maxX || n.y + r < minY || n.y - r > maxY) continue;
      visibleNodes.push(n);
    }

    const labelStride = Math.max(1, Math.ceil(visibleNodes.length / 500));
    const rootPath = new Path2D();
    const leafPath = new Path2D();
    const nodePath = new Path2D();
    for (const n of visibleNodes) {
      const r = nodeRadius(n);
      const path = n.isRoot ? rootPath : n.isLeaf ? leafPath : nodePath;
      path.moveTo(n.x + r, n.y);
      path.arc(n.x, n.y, r, 0, 2 * Math.PI);
    }
    ctx.fillStyle = NODE_COLOR;
    ctx.fill(nodePath);
    ctx.fillStyle = ROOT_COLOR;
    ctx.fill(rootPath);
    ctx.fillStyle = LEAF_COLOR;
    ctx.fill(leafPath);

    for (const n of visibleNodes) {
      const r = nodeRadius(n);
      const isSelected = selectedFnode === n.id;
      const isHovered = hoveredNode?.id === n.id;
      if (isSelected) {
        ctx.beginPath();
        ctx.arc(n.x, n.y, r, 0, 2 * Math.PI);
        ctx.strokeStyle = "#e7edf6";
        ctx.lineWidth = 2.5 / viewK;
        ctx.stroke();
      } else if (isHovered) {
        ctx.beginPath();
        ctx.arc(n.x, n.y, r, 0, 2 * Math.PI);
        ctx.strokeStyle = "#e7edf6";
        ctx.lineWidth = 1.5 / viewK;
        ctx.stroke();
      }
    }

    if (showShortFnode) {
      ctx.font = `${10 / viewK}px monospace`;
      ctx.fillStyle = `rgba(135, 147, 165, ${labelAlpha * 0.9})`;
      ctx.textAlign = "center";
      for (let index = 0; index < visibleNodes.length; index++) {
        const n = visibleNodes[index]!;
        const isSelected = selectedFnode === n.id;
        const isHovered = hoveredNode?.id === n.id;
        if (!isSelected && !isHovered && (!showLabels || index % labelStride !== 0)) continue;
        const r = nodeRadius(n);
        ctx.fillText(shortFnode(n.id), n.x, n.y + r + 10 / viewK);
      }
    }

    ctx.font = `${11 / viewK}px sans-serif`;
    ctx.textAlign = "left";
    for (let index = 0; index < visibleNodes.length; index++) {
      const n = visibleNodes[index]!;
      const isSelected = selectedFnode === n.id;
      const isHovered = hoveredNode?.id === n.id;
      const showNodeLabel = isSelected || isHovered || (showLabels && index % labelStride === 0);
      if (!showNodeLabel) continue;
      const r = nodeRadius(n);
      ctx.fillStyle = isSelected || isHovered
        ? "#e7edf6"
        : `rgba(192, 202, 216, ${labelAlpha * 0.82})`;
      ctx.fillText(n.label, n.x + r + 3 / viewK, n.y + 3 / viewK);
    }
  }

  function truncate(s: string, max: number): string {
    const characters = Array.from(s);
    return characters.length > max ? characters.slice(0, max - 1).join("") + "…" : s;
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
    const width = Math.max(1, Math.round(rect.width * dpr));
    const height = Math.max(1, Math.round(rect.height * dpr));
    if (canvas.width !== width) canvas.width = width;
    if (canvas.height !== height) canvas.height = height;
    canvas.style.width = `${rect.width}px`;
    canvas.style.height = `${rect.height}px`;
    // Redraw on the next frame so showing the view is not blocked by an
    // O(nodes + edges) paint. The grid remains visible beneath the canvas.
    requestRender();
  }

  function watchDevicePixelRatio() {
    dprQuery?.removeEventListener("change", watchDevicePixelRatio);
    dprQuery = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    dprQuery.addEventListener("change", watchDevicePixelRatio);
    resizeCanvas();
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
    if (nodes.length > 500 && nodeSpatialIndex.size > 0) {
      candidates = [];
      const cellX = Math.floor(wx / SPATIAL_CELL_SIZE);
      const cellY = Math.floor(wy / SPATIAL_CELL_SIZE);
      for (let dx = -1; dx <= 1; dx++) {
        for (let dy = -1; dy <= 1; dy++) {
          const bucket = nodeSpatialIndex.get(`${cellX + dx}:${cellY + dy}`);
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
    if (!canvas || e.button !== 0 || !e.isPrimary || activePointerId !== null) return;
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
      if (!mouseMoved) mouseMoved = Math.hypot(x - panStart.x, y - panStart.y) > 3;
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
    const node = e && !cancelled ? findNodeAt(x, y) : null;
    const hoverChanged = (node?.id ?? null) !== (hoveredNode?.id ?? null);
    hoveredNode = node;
    canvas.style.cursor = node ? "pointer" : "grab";
    if (hoverChanged) requestRender();
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

  function onLostPointerCapture(e: PointerEvent) {
    if (e.pointerId === activePointerId && mouseMode !== "idle") finishPointer(null, true);
  }

  function onPointerLeave() {
    if (mouseMode !== "idle" || !hoveredNode) return;
    hoveredNode = null;
    if (canvasEl) canvasEl.style.cursor = "grab";
    requestRender();
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
    const deltaY = e.deltaMode === WheelEvent.DOM_DELTA_LINE
      ? e.deltaY * 16
      : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
        ? e.deltaY * Math.max(canvas.clientHeight, 1)
        : e.deltaY;
    const factor = Math.exp(-deltaY * 0.0015);
    const nextScale = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, viewK * factor));
    if (nextScale === viewK) return;
    viewK = nextScale;

    // Adjust viewX/viewY so the world point under the cursor stays fixed.
    viewX = x - worldBefore.x * viewK;
    viewY = y - worldBefore.y * viewK;
    requestRender();
  }

  function fitToNodes() {
    const canvas = canvasEl;
    if (!canvas || nodes.length === 0) return;
    const { minX, maxX, minY, maxY } = graphBounds;
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
    requestRender();
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

  function fitGraphWhenMeasurable() {
    if (canvasEl && canvasEl.clientWidth > 0 && canvasEl.clientHeight > 0) {
      needsFit = false;
      fitToNodes();
    } else {
      needsFit = true;
    }
  }

  function ensureGraphLoaded(): Promise<void> {
    if (!running) return Promise.resolve();
    if (graphLoadPromise) return graphLoadPromise;
    if (graphInitialized && !graphDirty) return Promise.resolve();

    let allowImmediateRetry = true;
    graphLoadPromise = (async () => {
      if (!graphInitialized) {
        const loaded = await loadGraph();
        if (!loaded) {
          graphDirty = false;
          return;
        }
        if (!running) return;
        graphInitialized = true;
        fitGraphWhenMeasurable();
      }
      if (graphDirty) {
        const wasEmpty = nodes.length === 0;
        const reloadRevision = loadedRevision;
        graphDirty = false;
        if (!await reloadGraph()) {
          graphDirty = true;
          allowImmediateRetry = loadedRevision !== reloadRevision;
          return;
        }
        if (wasEmpty && nodes.length > 0) fitGraphWhenMeasurable();
      }
      if (!running) return;
      resizeCanvas();
      requestRender();
    })().finally(() => {
      graphLoadPromise = null;
      if (running && active && graphDirty && allowImmediateRetry) void ensureGraphLoaded();
    });
    return graphLoadPromise;
  }

  onMount(() => {
    watchDevicePixelRatio();
    if (canvasEl) {
      canvasEl.style.cursor = "grab";
      // Wheel must be registered with passive:false so preventDefault works.
      canvasEl.addEventListener("wheel", onWheel, { passive: false });
    }
    if (containerEl) {
      resizeObserver = new ResizeObserver(() => {
        resizeCanvas();
        // Fit after any layout change that gives the canvas usable dimensions.
        if (needsFit && canvasEl && canvasEl.clientWidth > 0 && canvasEl.clientHeight > 0) {
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
    dprQuery?.removeEventListener("change", watchDevicePixelRatio);
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
  let loadedRevision: number | null = null;
  $effect(() => {
    const nextRevision = revision;
    if (loadedRevision === null) {
      loadedRevision = nextRevision;
      return;
    }
    if (nextRevision === loadedRevision) return;
    loadedRevision = nextRevision;
    if (!graphInitialized) {
      if (graphLoadPromise) graphDirty = true;
      else if (untrack(() => active)) void ensureGraphLoaded();
      return;
    }
    graphDirty = true;
    if (untrack(() => active)) void ensureGraphLoaded();
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
    onlostpointercapture={onLostPointerCapture}
    onpointerleave={onPointerLeave}
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
