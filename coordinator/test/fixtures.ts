import {
  addGoal,
  newBoard,
  newProjectBoard,
  type Identity,
  type Board,
} from "../src/domain.js";
export const identity = (key: string): Identity => ({
  key,
  base_fingerprint: "base",
  local_context_hash: "ctx",
  relevant_options_hash: "opts",
});
export function board(id = "project"): Board {
  const b = newBoard(
    id,
    "alice",
    {
      title: "Shared proof",
      proposition: "True",
      base_key: 1,
      context: {},
      options: {},
      members: ["bob"],
      budget: 100,
      minimum_trust: "audited",
    },
    { base_key: 1 },
    identity("T"),
  );
  b.ready = true;
  return b;
}
/** A ready project board whose branch `main` is bound and whose root goal is T. */
export function projectBoard(id = "project"): Board {
  const b = newProjectBoard(
    id,
    "alice",
    {
      document: {
        database: "docs",
        branch: "main",
        node: "00000000-0000-4000-8000-000000000001",
      },
      members: ["bob"],
      budget: 100,
    },
    "Shared proof",
    {
      base: { base_key: 1 },
      context: {},
      options: {},
      minimum_trust: "audited",
      environment_id: "env",
    },
  );
  b.root = "T";
  addGoal(b, "T", identity("T"), "T");
  b.branches!.main.request.bound = true;
  b.ready = true;
  return b;
}
export function fact(
  id: string,
  goal: string,
  premises: string[] = [],
  project = "project",
) {
  return {
    id,
    goal,
    premises,
    trust: "trusted",
    request: { goal: identity(goal) },
    access: { visibility: "project", project_id: project },
    source: "theorem target : True := by trivial",
    receipt_id: "receipt",
  };
}
