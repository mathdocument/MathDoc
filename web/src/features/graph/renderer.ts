import type { FormalCodeStatus, GraphFull } from "../../lib/types";
import type { Theme } from "../../lib/theme";
import { shortFnode } from "../../lib/format";

export interface GraphAppearance {
  active: boolean;
  theme: Theme;
  selectedFnode: string | null;
}

/** Canvas ownership stays outside React: pointer frames never update component state. */
export function createGraphRenderer(
  canvasEl: HTMLCanvasElement,
  containerEl: HTMLDivElement,
  onSelect: (id: string | null) => void,
) {
  let active = false;
  let theme: Theme = "light";
  let selectedFnode: string | null = null;
  interface SimNode {
    id: string;
    name: string;
    labelLines: string[] | null;
    depth: number;
    inDegree: number;
    outDegree: number;
    lean: FormalCodeStatus;
    baseRadius: number;
    order: number;
    x: number;
    y: number;
  }

  let nodes: SimNode[] = [];
  let rafId = 0;
  let running = true;
  let hoveredNode: SimNode | null = null;

  // Pan / zoom. viewX/viewY are the screen position of world origin (0,0).
  // This is independent of canvas size, so resizing the canvas never moves
  // the graph — it just gives more/less visible area around the same origin.
  let viewX = 0;
  let viewY = 0;
  let viewK = 1;

  // Canvas colors mirror the tokens in design.css.
  const DARK_PALETTE = {
    unverified: "#8e8e8e",
    sorry: "#ff6f85",
    conditional: "#f0b661",
    verified: "#4fd6ae",
    edge: "142, 142, 142",
    outline: "#f0f0f0",
    shortLabel: "142, 142, 142",
    label: "212, 212, 212",
  };
  const LIGHT_PALETTE = {
    unverified: "#707070",
    sorry: "#c8384d",
    conditional: "#96601a",
    verified: "#0a7d64",
    edge: "112, 112, 112",
    outline: "#202020",
    shortLabel: "97, 97, 97",
    label: "69, 69, 69",
  };
  const MAX_NODE_RADIUS = 24;
  const SPATIAL_CELL_SIZE = 64;
  const MIN_ZOOM = 0.0001;
  const MAX_ZOOM = 5;
  const MAX_CANVAS_DPR = 2;
  const MAX_BACKING_PIXELS = 8_000_000;

  let nodeSpatialIndex = new Map<string, SimNode[]>();
  let nodesById = new Map<string, SimNode>();
  let nodesByX: SimNode[] = [];
  let outgoingLinks = new Map<string, SimNode[]>();
  let incomingLinks = new Map<string, SimNode[]>();
  let selfEdgeNodes = new Set<string>();
  let dprQuery: MediaQueryList | null = null;
  let canvasDpr = 1;
  let canvasCssWidth = 0;
  let canvasCssHeight = 0;
  let edgePathSelection: string | null | undefined;
  let selectedOutgoingPath = new Path2D();
  let selectedIncomingPath = new Path2D();
  let graphBounds = {
    minX: Infinity,
    maxX: -Infinity,
    minY: Infinity,
    maxY: -Infinity,
  };

  function nodeRadius(n: SimNode, selection: string | null): number {
    const r = n.baseRadius;
    if (selection && n.id === selection) return r + 3;
    if (hoveredNode && n.id === hoveredNode.id) return r + 2;
    return r;
  }

  // Compute directed degrees and node sizes.
  function computeMetadata(nodeList: SimNode[], edges: GraphFull["edges"]) {
    outgoingLinks = new Map<string, SimNode[]>();
    incomingLinks = new Map<string, SimNode[]>();
    selfEdgeNodes = new Set<string>();
    for (const [sourceIndex, targetIndex] of edges) {
      const source = nodeList[sourceIndex]!;
      const target = nodeList[targetIndex]!;
      const s = source.id;
      const t = target.id;
      if (source === target) selfEdgeNodes.add(s);
      source.outDegree++;
      target.inDegree++;
      const outgoing = outgoingLinks.get(s);
      if (outgoing) outgoing.push(target);
      else outgoingLinks.set(s, [target]);
      const incoming = incomingLinks.get(t);
      if (incoming) incoming.push(source);
      else incomingLinks.set(t, [source]);
    }
    for (const n of nodeList) {
      const ior = Math.log1p(n.inDegree) - Math.log1p(n.outDegree);
      n.baseRadius = Math.min(MAX_NODE_RADIUS, 6 * (Math.max(0, ior) + 1));
    }
  }

  function installGraph(data: GraphFull) {
    nodes = data.nodes.map((node) => ({
      id: node.fnode,
      name: node.name,
      labelLines: null,
      depth: node.depth,
      inDegree: 0,
      outDegree: 0,
      lean: node.lean ?? "unverified",
      baseRadius: 6,
      order: 0,
      x: 0,
      y: 0,
    }));
    nodesById = new Map(nodes.map((node) => [node.id, node]));
    computeMetadata(nodes, data.edges);
    applyStaticGraphLayout();
    computeGraphBounds();
    buildSpatialIndexes();
    edgePathSelection = undefined;
    selectedOutgoingPath = new Path2D();
    selectedIncomingPath = new Path2D();
    hoveredNode = null;
    if (canvasEl) canvasEl.style.cursor = "grab";
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
    const rowsPerColumn = Math.max(
      24,
      Math.ceil(Math.sqrt(nodes.length) * 1.5),
    );
    const spacing = 58;
    let cursorX = 0;
    nodesByX = [];
    for (const depth of depths) {
      const layer = layers.get(depth)!;
      layer.sort(
        (a, b) => lexicalCompare(a.name, b.name) || lexicalCompare(a.id, b.id),
      );
      const columns = Math.ceil(layer.length / rowsPerColumn);
      const rows = Math.min(rowsPerColumn, layer.length);
      for (let index = 0; index < layer.length; index++) {
        const node = layer[index]!;
        const column = Math.floor(index / rowsPerColumn);
        const row = index % rowsPerColumn;
        node.x = cursorX + column * spacing;
        node.y = (row - (rows - 1) / 2) * spacing;
        node.order = nodesByX.length;
        nodesByX.push(node);
      }
      cursorX += Math.max(140, columns * spacing + 90);
    }

    const offsetX = cursorX / 2;
    for (const node of nodes) node.x = (node.x ?? 0) - offsetX;
  }

  function computeGraphBounds() {
    let minX = Infinity,
      maxX = -Infinity,
      minY = Infinity,
      maxY = -Infinity;
    for (const node of nodes) {
      minX = Math.min(minX, node.x - node.baseRadius);
      maxX = Math.max(maxX, node.x + node.baseRadius);
      minY = Math.min(minY, node.y - node.baseRadius);
      maxY = Math.max(maxY, node.y + node.baseRadius);
      if (selfEdgeNodes.has(node.id)) {
        const loopRadius = node.baseRadius + 12;
        minX = Math.min(minX, node.x - loopRadius);
        maxX = Math.max(maxX, node.x + loopRadius);
        minY = Math.min(minY, node.y - 2 * loopRadius);
      }
    }
    graphBounds = { minX, maxX, minY, maxY };
  }

  function buildSpatialIndexes() {
    nodeSpatialIndex = new Map();
    for (const node of nodes) {
      const key = spatialKey(node.x, node.y);
      const bucket = nodeSpatialIndex.get(key);
      if (bucket) bucket.push(node);
      else nodeSpatialIndex.set(key, [node]);
    }
  }

  function requestRender() {
    if (rafId || !running || !active) return;
    rafId = requestAnimationFrame(() => {
      rafId = 0;
      if (!running || !active) return;
      // Changing the backing size clears the bitmap. Resize and paint in the
      // same frame; a ResizeObserver reset would expose a blank frame first.
      resizeCanvas();
      render();
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

  function appendEdge(
    path: Path2D,
    source: SimNode,
    target: SimNode,
    reciprocal: boolean,
    loopOffset = 0,
  ) {
    if (source === target) {
      const loopRadius = source.baseRadius + 8 + loopOffset;
      path.moveTo(source.x, source.y);
      path.arc(
        source.x,
        source.y - loopRadius,
        loopRadius,
        Math.PI / 2,
        Math.PI * 2.5,
      );
    } else if (reciprocal) {
      const dx = target.x - source.x;
      const dy = target.y - source.y;
      const length = Math.hypot(dx, dy) || 1;
      const curve = 10;
      path.moveTo(source.x, source.y);
      path.quadraticCurveTo(
        (source.x + target.x) / 2 - (dy / length) * curve,
        (source.y + target.y) / 2 + (dx / length) * curve,
        target.x,
        target.y,
      );
    } else {
      path.moveTo(source.x, source.y);
      path.lineTo(target.x, target.y);
    }
  }

  function ensureSelectedEdgePaths(selection: string | null) {
    if (edgePathSelection === selection) return;
    edgePathSelection = selection;
    selectedOutgoingPath = new Path2D();
    selectedIncomingPath = new Path2D();
    if (!selection) return;
    const selected = nodesById.get(selection);
    if (!selected) return;
    const outgoing = outgoingLinks.get(selection) ?? [];
    const incoming = incomingLinks.get(selection) ?? [];
    const incomingIds = new Set(incoming.map((node) => node.id));
    const outgoingIds = new Set(outgoing.map((node) => node.id));
    for (const target of outgoing) {
      appendEdge(
        selectedOutgoingPath,
        selected,
        target,
        incomingIds.has(target.id),
      );
    }
    for (const source of incoming) {
      appendEdge(
        selectedIncomingPath,
        source,
        selected,
        outgoingIds.has(source.id),
        source === selected ? 4 : 0,
      );
    }
  }

  function render() {
    const canvas = canvasEl;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const dpr = canvasDpr;
    const w = canvas.width / dpr;
    const h = canvas.height / dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    // viewX/viewY = screen position of world origin. Not relative to canvas
    // center, so resizing the canvas doesn't move the graph.
    // Keep thin edges on the same backing-pixel phase while panning.
    ctx.translate(Math.round(viewX * dpr) / dpr, Math.round(viewY * dpr) / dpr);
    ctx.scale(viewK, viewK);

    const margin = 80 / viewK;
    const minX = -viewX / viewK - margin;
    const maxX = (w - viewX) / viewK + margin;
    const minY = -viewY / viewK - margin;
    const maxY = (h - viewY) / viewK + margin;

    // LOD thresholds.
    const showLabels = viewK > 0.9;
    const graphSelection =
      selectedFnode && nodesById.has(selectedFnode) ? selectedFnode : null;
    const palette = theme === "light" ? LIGHT_PALETTE : DARK_PALETTE;

    ensureSelectedEdgePaths(graphSelection);
    if (graphSelection) {
      const selected = nodesById.get(graphSelection)!;
      const selectedDegree = selected.inDegree + selected.outDegree;
      const highlightAlpha = Math.max(
        0.77,
        Math.min(0.9, 12 / Math.sqrt(Math.max(1, selectedDegree))),
      );
      ctx.lineWidth = (selectedDegree > 1_000 ? 1.25 : 2) / viewK;
      ctx.setLineDash([]);
      ctx.strokeStyle = `rgba(${palette.edge}, ${highlightAlpha})`;
      ctx.stroke(selectedOutgoingPath);
      ctx.strokeStyle = `rgba(${palette.edge}, ${highlightAlpha})`;
      ctx.stroke(selectedIncomingPath);
    }

    // Keep only visible nodes for drawing, labels, and pointer hit testing.
    const visibleNodes: SimNode[] = [];
    const nodeMargin = MAX_NODE_RADIUS + 3;
    const firstNode = firstNodeAtOrAfter(minX - nodeMargin);
    for (let index = firstNode; index < nodesByX.length; index++) {
      const n = nodesByX[index]!;
      if (n.x > maxX + nodeMargin) break;
      const r = nodeRadius(n, graphSelection);
      if (n.x + r < minX || n.x - r > maxX || n.y + r < minY || n.y - r > maxY)
        continue;
      visibleNodes.push(n);
    }

    const labelStride = Math.max(1, Math.ceil(visibleNodes.length / 500));
    const paths = {
      unverified: new Path2D(),
      sorry: new Path2D(),
      conditional: new Path2D(),
      verified: new Path2D(),
    };
    for (const n of visibleNodes) {
      const r = nodeRadius(n, graphSelection);
      const path = paths[n.lean];
      path.moveTo(n.x + r, n.y);
      path.arc(n.x, n.y, r, 0, 2 * Math.PI);
    }
    for (const status of [
      "unverified",
      "sorry",
      "conditional",
      "verified",
    ] as const) {
      ctx.fillStyle = palette[status];
      ctx.globalAlpha = 0.12;
      ctx.fill(paths[status]);
      ctx.globalAlpha = 0.85;
      ctx.strokeStyle = palette[status];
      ctx.lineWidth = Math.min(1.8 / viewK, 2.5);
      ctx.setLineDash(status === "conditional" ? [3 / viewK, 3 / viewK] : []);
      ctx.stroke(paths[status]);
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;
    }

    // A solid core keeps status legible when zoomed out; the outer ring adds structure.
    const cores = {
      unverified: new Path2D(),
      sorry: new Path2D(),
      conditional: new Path2D(),
      verified: new Path2D(),
    };
    for (const node of visibleNodes) {
      const radius = nodeRadius(node, graphSelection) * 0.36;
      cores[node.lean].moveTo(node.x + radius, node.y);
      cores[node.lean].arc(node.x, node.y, radius, 0, Math.PI * 2);
    }
    for (const status of [
      "unverified",
      "sorry",
      "conditional",
      "verified",
    ] as const) {
      ctx.fillStyle = palette[status];
      ctx.fill(cores[status]);
    }

    const selectedNode = graphSelection
      ? (nodesById.get(graphSelection) ?? null)
      : null;
    if (selectedNode) {
      ctx.beginPath();
      ctx.arc(
        selectedNode.x,
        selectedNode.y,
        nodeRadius(selectedNode, graphSelection) + 5 / viewK,
        0,
        2 * Math.PI,
      );
      ctx.strokeStyle = palette.outline;
      ctx.lineWidth = 1.25 / viewK;
      ctx.stroke();
    }
    if (hoveredNode && hoveredNode.id !== graphSelection) {
      ctx.beginPath();
      ctx.arc(
        hoveredNode.x,
        hoveredNode.y,
        nodeRadius(hoveredNode, graphSelection),
        0,
        2 * Math.PI,
      );
      ctx.strokeStyle = palette.outline;
      ctx.lineWidth = 1.5 / viewK;
      ctx.stroke();
    }

    const labelNodes: SimNode[] = [];
    const labelIds = new Set<string>();
    const addLabelNode = (node: SimNode | null) => {
      if (!node || labelIds.has(node.id)) return;
      labelIds.add(node.id);
      labelNodes.push(node);
    };
    if (showLabels) {
      for (const node of visibleNodes) {
        if (node.order % labelStride === 0) addLabelNode(node);
        if (labelNodes.length >= 500) break;
      }
    }
    addLabelNode(selectedNode);
    addLabelNode(hoveredNode);

    if (showLabels) {
      ctx.font = `${10 / viewK}px monospace`;
      ctx.fillStyle = `rgba(${palette.shortLabel}, 0.9)`;
      ctx.textAlign = "right";
      ctx.textBaseline = "middle";
      for (const n of labelNodes) {
        const r = nodeRadius(n, graphSelection);
        ctx.fillText(shortFnode(n.id), n.x - r - 5 / viewK, n.y);
      }
    }

    ctx.font = `${11 / viewK}px sans-serif`;
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    for (const n of labelNodes) {
      const isSelected = selectedFnode === n.id;
      const isHovered = hoveredNode?.id === n.id;
      const r = nodeRadius(n, graphSelection);
      ctx.fillStyle =
        isSelected || isHovered
          ? palette.outline
          : `rgba(${palette.label}, 0.82)`;
      const labelY = n.y + r + 5 / viewK;
      const labelLines = (n.labelLines ??= wrapLabel(n.name, 16));
      for (let line = 0; line < labelLines.length; line++) {
        ctx.fillText(labelLines[line]!, n.x, labelY + (line * 13) / viewK);
      }
    }
  }

  function wrapLabel(value: string, maxLineLength: number): string[] {
    const characters = Array.from(value.trim().replace(/\s+/g, " "));
    if (characters.length <= maxLineLength) return [characters.join("")];

    let firstEnd = maxLineLength;
    for (
      let index = maxLineLength - 1;
      index >= Math.ceil(maxLineLength * 0.6);
      index--
    ) {
      if (/\s/.test(characters[index]!)) {
        firstEnd = index;
        break;
      }
    }
    const first = characters.slice(0, firstEnd).join("").trimEnd();
    const remainder = characters.slice(firstEnd).join("").trimStart();
    const rest = Array.from(remainder);
    const second =
      rest.length > maxLineLength
        ? rest.slice(0, maxLineLength - 3).join("") + "..."
        : remainder;
    return second ? [first, second] : [first];
  }

  // ── Canvas sizing ───────────────────────────────────────────────────────────

  function resizeCanvas() {
    const canvas = canvasEl;
    const container = containerEl;
    if (!canvas || !container) return;
    canvasCssWidth = container.clientWidth;
    canvasCssHeight = container.clientHeight;
    const cssPixels = Math.max(1, canvasCssWidth * canvasCssHeight);
    const dpr = Math.min(
      window.devicePixelRatio || 1,
      MAX_CANVAS_DPR,
      Math.sqrt(MAX_BACKING_PIXELS / cssPixels),
    );
    canvasDpr = dpr;
    const width = Math.max(1, Math.round(canvasCssWidth * dpr));
    const height = Math.max(1, Math.round(canvasCssHeight * dpr));
    if (canvas.width !== width) canvas.width = width;
    if (canvas.height !== height) canvas.height = height;
  }

  function watchDevicePixelRatio() {
    dprQuery?.removeEventListener("change", watchDevicePixelRatio);
    dprQuery = window.matchMedia(
      `(resolution: ${window.devicePixelRatio}dppx)`,
    );
    dprQuery.addEventListener("change", watchDevicePixelRatio);
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
    const selection = selectedFnode;
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
      const dx = n.x - wx;
      const dy = n.y - wy;
      const r = nodeRadius(n, selection) + 4;
      if (dx * dx + dy * dy <= r * r) return n;
    }
    return null;
  }

  let panStart: {
    pointerId: number;
    x: number;
    y: number;
    viewX: number;
    viewY: number;
    pressedNode: SimNode | null;
    moved: boolean;
  } | null = null;

  function onPointerDown(e: PointerEvent) {
    const canvas = canvasEl;
    if (!canvas || e.button !== 0 || !e.isPrimary || panStart) return;
    canvas.setPointerCapture(e.pointerId);
    const rect = canvas.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    panStart = {
      pointerId: e.pointerId,
      x,
      y,
      viewX,
      viewY,
      pressedNode: findNodeAt(x, y),
      moved: false,
    };
  }

  function onPointerMove(e: PointerEvent) {
    const canvas = canvasEl;
    if (!canvas) return;
    if (panStart && e.pointerId !== panStart.pointerId) return;
    const rect = canvas.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;

    if (panStart) {
      if (!panStart.moved)
        panStart.moved = Math.hypot(x - panStart.x, y - panStart.y) > 3;
      if (panStart.moved) {
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
    const start = panStart;

    if (!cancelled && start && !start.moved) {
      if (start.pressedNode) {
        onSelect(
          start.pressedNode.id === selectedFnode ? null : start.pressedNode.id,
        );
      } else if (selectedFnode) {
        onSelect(null);
      }
    }
    panStart = null;
    const node = e && !cancelled ? findNodeAt(x, y) : null;
    const hoverChanged = (node?.id ?? null) !== (hoveredNode?.id ?? null);
    hoveredNode = node;
    canvas.style.cursor = node ? "pointer" : "grab";
    if (hoverChanged) requestRender();
    if (start && canvas.hasPointerCapture(start.pointerId)) {
      canvas.releasePointerCapture(start.pointerId);
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (e.pointerId === panStart?.pointerId) finishPointer(e, false);
  }

  function onPointerCancel(e: PointerEvent) {
    if (e.pointerId === panStart?.pointerId) finishPointer(e, true);
  }

  function onLostPointerCapture(e: PointerEvent) {
    if (e.pointerId === panStart?.pointerId) finishPointer(null, true);
  }

  function onPointerLeave() {
    if (panStart || !hoveredNode) return;
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
    const deltaY =
      e.deltaMode === WheelEvent.DOM_DELTA_LINE
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
    const container = containerEl;
    if (!container || nodes.length === 0) return;
    const { minX, maxX, minY, maxY } = graphBounds;
    if (minX === Infinity) return;
    const graphW = maxX - minX;
    const graphH = maxY - minY;
    const cx = (minX + maxX) / 2;
    const cy = (minY + maxY) / 2;
    const cw = container.clientWidth;
    const ch = container.clientHeight;
    const margin = 0.2;
    const scaleX = cw / (graphW + 2 * 20);
    const scaleY = ch / (graphH + 2 * 20);
    viewK = Math.max(
      MIN_ZOOM,
      Math.min(MAX_ZOOM, Math.min(scaleX, scaleY) * (1 - margin)),
    );
    viewX = cw / 2 - cx * viewK;
    viewY = ch / 2 - cy * viewK;
    requestRender();
  }

  /** Zoom by `factor` around the canvas center (used by the zoom buttons). */
  function zoomBy(factor: number) {
    const container = containerEl;
    if (!container) return;
    const cw = container.clientWidth;
    const ch = container.clientHeight;
    const anchor = screenToWorld(cw / 2, ch / 2);
    viewK = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, viewK * factor));
    viewX = cw / 2 - anchor.x * viewK;
    viewY = ch / 2 - anchor.y * viewK;
    requestRender();
  }

  let needsFit = true;
  const observer = new ResizeObserver(() => {
    if (
      canvasCssWidth !== containerEl.clientWidth ||
      canvasCssHeight !== containerEl.clientHeight
    )
      requestRender();
    if (needsFit && containerEl.clientWidth && containerEl.clientHeight) {
      needsFit = false;
      fitToNodes();
    }
  });
  const blur = () => finishPointer(null, true);
  canvasEl.style.cursor = "grab";
  watchDevicePixelRatio();
  observer.observe(containerEl);
  canvasEl.addEventListener("wheel", onWheel, { passive: false });
  canvasEl.addEventListener("pointerdown", onPointerDown);
  canvasEl.addEventListener("pointermove", onPointerMove);
  canvasEl.addEventListener("pointerup", onPointerUp);
  canvasEl.addEventListener("pointercancel", onPointerCancel);
  canvasEl.addEventListener("lostpointercapture", onLostPointerCapture);
  canvasEl.addEventListener("pointerleave", onPointerLeave);
  window.addEventListener("blur", blur);
  window.addEventListener("resize", requestRender);
  return {
    setGraph(data: GraphFull) {
      const wasEmpty = nodes.length === 0;
      installGraph(data);
      if (wasEmpty) {
        needsFit = true;
        if (containerEl.clientWidth && containerEl.clientHeight) {
          fitToNodes();
          needsFit = false;
        }
      }
      requestRender();
    },
    update(next: GraphAppearance) {
      active = next.active;
      theme = next.theme;
      selectedFnode = next.selectedFnode;
      if (active) requestRender();
      else {
        finishPointer(null, true);
        stopRaf();
        canvasEl.width = 1;
        canvasEl.height = 1;
      }
    },
    fit: fitToNodes,
    zoom: zoomBy,
    destroy() {
      running = false;
      finishPointer(null, true);
      stopRaf();
      observer.disconnect();
      dprQuery?.removeEventListener("change", watchDevicePixelRatio);
      canvasEl.removeEventListener("wheel", onWheel);
      canvasEl.removeEventListener("pointerdown", onPointerDown);
      canvasEl.removeEventListener("pointermove", onPointerMove);
      canvasEl.removeEventListener("pointerup", onPointerUp);
      canvasEl.removeEventListener("pointercancel", onPointerCancel);
      canvasEl.removeEventListener("lostpointercapture", onLostPointerCapture);
      canvasEl.removeEventListener("pointerleave", onPointerLeave);
      window.removeEventListener("blur", blur);
      window.removeEventListener("resize", requestRender);
    },
  };
}
export type GraphRenderer = ReturnType<typeof createGraphRenderer>;
