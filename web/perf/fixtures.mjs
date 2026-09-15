export const GRAPH_NODE_COUNT = 10_000;
export const GRAPH_EDGE_COUNT = (GRAPH_NODE_COUNT - 1) + (GRAPH_NODE_COUNT - 6);
export const EDITOR_LINE_COUNT = 500;
export const RELATION_COUNT = 8_311;

const rootFnode = "perf-root";

const latexSource = Array.from(
  { length: EDITOR_LINE_COUNT },
  (_, index) => index === 0
    ? "\\begin{proof}Inline proof.\\end{proof} Outside proof."
    : `\\paragraph{Case ${index + 1}.} If $x_{${index + 1}} \\in \\mathbb{R}$, then $x_{${index + 1}}^2 \\ge 0$.`,
).join("\n");

function summary(index = 0) {
  return {
    fnode: index === 0 ? rootFnode : `perf-node-${String(index).padStart(5, "0")}`,
    title: index === 0 ? "Performance fixture" : `Deterministic graph node ${index}`,
    broken: false,
    depth: Math.floor(Math.log2(index + 1)),
  };
}

function nodeView(withEditor) {
  return {
    node: {
      ...summary(),
      revision: "perf-revision",
      depens: [],
      blocks: withEditor
        ? [{ srctype: "latex", content: latexSource, metadata: {} }]
        : [],
      formalization: { lean: "no_code", rocq: "no_code" },
    },
    referrers: [],
    children: [],
  };
}

function graph() {
  const nodes = Array.from({ length: GRAPH_NODE_COUNT }, (_, index) => summary(index));
  const edges = [];
  for (let index = 1; index < GRAPH_NODE_COUNT; index++) {
    edges.push([Math.floor((index - 1) / 2), index]);
    if (index >= 6) edges.push([Math.floor((index - 3) / 3), index]);
  }
  return { nodes, edges };
}

export function apiBodies(scenario) {
  const fullGraph = scenario !== "editor" ? graph() : { nodes: [summary()], edges: [] };
  if (scenario === "graph" && fullGraph.edges.length !== GRAPH_EDGE_COUNT) {
    throw new Error("graph fixture edge count is stale");
  }
  const view = nodeView(scenario === "editor");
  if (scenario === "relations") {
    view.children = Array.from({ length: RELATION_COUNT }, (_, i) => ({
      ...summary(i + 1), formalization: { lean: i % 2 ? "verified" : "unverified", rocq: "no_code" },
    }));
    view.node.depens = view.children.map(node => node.fnode);
  }
  const bodies = new Map([
    ["/api/resolve", { fnode: rootFnode }],
    ["/api/graph/roots", [{ ...summary(), component_size: GRAPH_NODE_COUNT, topo_depth: 0 }]],
    ["/api/graph/check", {
      nodes: scenario === "graph" ? GRAPH_NODE_COUNT : 1,
      edges: scenario === "graph" ? GRAPH_EDGE_COUNT : 0,
      missing: [],
      invalid: [],
      cycles: [],
    }],
    ["/api/graph/full", fullGraph],
    [`/api/node/${rootFnode}/view`, view],
    [`/api/node/${summary(RELATION_COUNT).fnode}/view`, {
      node: { ...view.node, ...summary(RELATION_COUNT), depens: [], blocks: [] },
      children: [], referrers: [view.node],
    }],
  ]);
  return new Map([...bodies].map(([path, body]) => [path, JSON.stringify(body)]));
}
