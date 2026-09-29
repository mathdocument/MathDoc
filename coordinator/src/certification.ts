// The formal-certification dimension of a node's status (plan §7.1), computed from the
// ProofRequests bound to its branch. Local checks are a separate dimension.
import { leanContentSha256 } from "./contracts.js";
import type { Board } from "./domain.js";
import type { Node } from "./docs/model.js";

export type LeanCertification =
  | null
  | { status: "not_submitted" }
  | {
      status: "insufficient" | "derivable" | "certified" | "stale" | "rejected";
      project: string;
      goal?: string;
      minimum_trust: string;
      checked_at: string | null;
      /** Local derivation over project-visible facts is never truncated. */
      truncated: boolean;
      reason?: string;
      details?: string;
    };

export type CertificationLookup = (node: Node) => LeanCertification;

/** Latest bound project per node wins (highest revision). */
export function certificationLookup(
  boards: Board[],
  nodes: Map<string, Node>,
): CertificationLookup {
  const latest = new Map<string, Board>();
  for (const b of boards)
    for (const id of Object.keys(b.nodes ?? {})) {
      const prior = latest.get(id);
      if (!prior || prior.revision < b.revision) latest.set(id, b);
    }
  const sha = (id: string) => {
    const n = nodes.get(id);
    return n ? leanContentSha256(n) : null;
  };
  return (node) => {
    if (!node.blocks.some((b) => b.srctype === "lean")) return null;
    const b = latest.get(node.fnode);
    if (!b) return { status: "not_submitted" };
    const binding = b.nodes![node.fnode];
    const base = {
      project: b.id,
      minimum_trust: b.minimum_trust,
      checked_at: b.last_sync ? new Date(b.last_sync).toISOString() : null,
      truncated: false,
    };
    const changed =
      sha(node.fnode) !== binding.lean_content_sha256 ||
      (binding.definition_nodes ?? []).some(
        (d) => sha(d) !== b.nodes![d]?.lean_content_sha256,
      );
    if (changed)
      return {
        ...base,
        status: "stale",
        details:
          "the Lean text or a definition it uses changed after submission",
      };
    if (binding.state === "rejected" || !binding.goal)
      return {
        ...base,
        status: "rejected",
        reason: binding.reason,
        details: binding.details,
      };
    return {
      ...base,
      status: b.goals[binding.goal]?.status ?? "insufficient",
      goal: binding.goal,
    };
  };
}
