// A node's Lean standing as the graph shows it (docs/mathdoc.md §4.5): its LeanGround
// certification, or for a definition its registration, or no Lean code. The words are the node
// page's (FormalStatus); the icons are the proof graph's: hollow while nothing is established,
// filled once LeanGround has, a diamond for a definition. An imported upstream file is covered by
// its import's project check, not by certification.
import type { GraphFull } from "./types";

export type NodeTone =
  | "no_code" | "imported" | "not_submitted" | "definition" | "registered"
  | "insufficient" | "derivable" | "certified" | "problem";

/** Legend order: from no Lean code to certified, then the ones needing attention. */
export const TONES: NodeTone[] = [
  "no_code", "imported", "definition", "registered", "not_submitted", "insufficient", "derivable", "certified", "problem",
];

export const toneLabel: Record<NodeTone, string> = {
  no_code: "No Lean code",
  imported: "Imported file",
  definition: "Definition",
  registered: "Registered definition",
  not_submitted: "Not submitted",
  insufficient: "Insufficient evidence",
  derivable: "Derivable",
  certified: "Certified",
  problem: "Stale or not accepted",
};

export function nodeTone(n: Pick<GraphFull["nodes"][number], "lean" | "lean_role" | "certification">): NodeTone {
  if (n.lean === "no_code") return "no_code";
  const c = n.certification;
  if (c === "imported") return "imported";
  if (c === "stale" || c === "rejected") return "problem";
  if (n.lean_role === "definition") return c === "registered" ? "registered" : "definition";
  if (c === "insufficient" || c === "derivable" || c === "certified") return c;
  return "not_submitted";
}

/** Definitions draw as diamonds, everything else as circles. */
export const isDefinitionTone = (tone: NodeTone) => tone === "definition" || tone === "registered";
