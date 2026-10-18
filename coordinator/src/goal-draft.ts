// Goal drafts (docs/mathdoc.md §2.7.2, §4.11): everything `mdc pull` writes into a goal file,
// for any goal of a project board, whether a document node states it or only a sketch did.
import { z } from "zod";
import {
  attached,
  requireThat,
  type Board,
  type Declaration,
} from "./domain.js";
import { convert, type Conversion } from "./proof.js";
import type { Snapshot } from "./docs/graph.js";

/** One goal stated as a theorem, with the definition IDs its statement may use. */
export interface GoalStatement extends Declaration {
  goal: string;
  definitions: string[];
  /**
   * Where the signature comes from, in this order: a converted node of the branch bound to the
   * goal; the declaration recorded on the board (a node's, or the sketch placeholder that
   * introduced the goal); the resolved proposition alone, as `theorem name : proposition`.
   */
  from: "node" | "declaration" | "proposition";
  node?: string;
}
export interface DefinitionSource {
  id: string;
  name: string;
  source: string;
}
export interface GoalDraft {
  project: string;
  revision: number;
  environment_id: string;
  goal: string;
  status: string;
  branch: string | null;
  root_module: string;
  context: Record<string, unknown>;
  options: Record<string, unknown>;
  root: GoalStatement;
  premises: GoalStatement[];
  /** Registered definitions used by the statements, dependencies first. */
  definitions: DefinitionSource[];
  /** The goal's latest DRAFT_NOTES attempt notes, newest first (§4.11 G2). */
  notes: { at: string; actor: string; text: string }[];
  /** The goal's prove task, with its lease when leased. */
  task: {
    id: string;
    state: string;
    epoch: number;
    attempt?: {
      id: string;
      actor: string;
      epoch: number;
      expires: number;
      reserved: number;
    };
  } | null;
}

const definitionRecord = z
  .object({
    id: z.string(),
    name: z.string(),
    source: z.string(),
    deps: z.array(z.string()).default([]),
  })
  .passthrough();

export const DRAFT_NOTES = 20;

export async function goalDraft(input: {
  board: Board;
  goal: string;
  branch?: string;
  with?: string[];
  snapshot: (branch: string) => Promise<Snapshot>;
  definition: (id: string) => Promise<unknown>;
}): Promise<GoalDraft> {
  const b = input.board;
  requireThat(b.project, "not_a_project_board", 400);
  const known = (key: string) => Object.hasOwn(b.goals, key);
  requireThat(known(input.goal), "unknown_goal", 404);
  const extra = [...new Set(input.with ?? [])].filter((k) => k !== input.goal);
  for (const k of extra) requireThat(known(k), "unknown_goal", 404);

  // Nodes of the branch that state a goal, from the branch's current text. A node edited since
  // its conversion may state something else, so only unchanged nodes count.
  let conversion: Conversion | undefined;
  const branch =
    input.branch !== undefined &&
    b.branches &&
    Object.hasOwn(b.branches, input.branch)
      ? attached(b, input.branch)
      : undefined;
  const nodeOf = async (key: string) => {
    if (!branch?.request.bound) return undefined;
    const ids = Object.keys(branch.nodes)
      .filter(
        (n) =>
          branch.nodes[n].goal === key && branch.nodes[n].role === "theorem",
      )
      .sort();
    if (!ids.length) return undefined;
    conversion ??= convert(
      (await input.snapshot(input.branch!)).nodes,
      branch.request.node,
      b.context as never,
    );
    return ids.find(
      (n) =>
        conversion!.bindings[n]?.role === "theorem" &&
        conversion!.converted.get(n)?.decl?.type &&
        conversion!.bindings[n].lean_content_sha256 ===
          branch.nodes[n].lean_content_sha256,
    );
  };
  const nodeDefinitions = (n: string) =>
    (branch!.nodes[n].definition_nodes ?? []).flatMap((d) =>
      Object.hasOwn(branch!.nodes, d)
        ? (branch!.nodes[d].definition_ids ?? [])
        : [],
    );
  const statement = async (key: string): Promise<GoalStatement> => {
    const g = b.goals[key];
    // Without an identity no proof of the goal can be submitted (`goal_identity_unavailable`).
    requireThat(g.identity, "goal_unresolved");
    const node = await nodeOf(key);
    if (node) {
      const d = conversion!.converted.get(node)!.decl!;
      return {
        goal: key,
        name: d.name,
        binders: d.binders ?? "",
        type: d.type!,
        definitions: nodeDefinitions(node),
        from: "node",
        node,
      };
    }
    if (g.declaration)
      return {
        goal: key,
        ...g.declaration,
        definitions: g.definitions ?? [],
        from: "declaration",
      };
    requireThat(g.proposition, "goal_unresolved");
    return {
      goal: key,
      name: /^[A-Za-z_][A-Za-z0-9_]*$/.test(g.statement)
        ? g.statement
        : `goal_${key.slice(0, 8)}`,
      binders: "",
      type: g.proposition,
      definitions: g.definitions ?? [],
      from: "proposition",
    };
  };

  const root = await statement(input.goal);
  // Available premises: the node's direct premise nodes, then the goals asked for.
  const premises: GoalStatement[] = [];
  const add = async (key: string | undefined) => {
    if (!key || key === input.goal || premises.some((p) => p.goal === key))
      return;
    premises.push(await statement(key));
  };
  if (root.node)
    for (const p of conversion!.bindings[root.node].premises ?? [])
      if (b.goals[branch!.nodes[p]?.goal ?? ""]?.identity)
        await add(branch!.nodes[p].goal);
  for (const k of extra) await add(k);

  const definitions: DefinitionSource[] = [];
  const seen = new Set<string>();
  const visit = async (id: string) => {
    if (seen.has(id)) return;
    seen.add(id);
    const d = definitionRecord.parse(await input.definition(id));
    for (const dep of [...d.deps].sort()) await visit(dep);
    definitions.push({ id: d.id, name: d.name, source: d.source });
  };
  for (const id of [
    ...new Set([root, ...premises].flatMap((s) => s.definitions)),
  ].sort())
    await visit(id);

  return {
    project: b.id,
    revision: b.revision,
    environment_id: b.project.environment_id,
    goal: input.goal,
    status: b.goals[input.goal].status,
    branch: branch ? input.branch! : null,
    root_module: String(b.base.root_module ?? ""),
    context: b.context,
    options: b.options,
    root,
    premises,
    definitions,
    ...draftWork(b, input.goal),
  };
}

/** What every goal draft carries besides the statement: the latest notes and the prove task. */
export function draftWork(
  b: Board,
  goal: string,
): Pick<GoalDraft, "notes" | "task"> {
  const task =
    Object.values(b.tasks).find(
      (t) => t.goal === goal && t.kind === "prove" && t.state !== "cancelled",
    ) ?? null;
  const attempt =
    task?.attempt && Object.hasOwn(b.attempts, task.attempt)
      ? b.attempts[task.attempt]
      : undefined;
  return {
    notes: (b.goals[goal].notes ?? [])
      .slice(-DRAFT_NOTES)
      .reverse()
      .map((n) => ({
        at: new Date(n.at).toISOString(),
        actor: n.actor,
        text: n.text,
      })),
    task: task && {
      id: task.id,
      state: task.state,
      epoch: task.epoch,
      ...(attempt
        ? {
            attempt: {
              id: attempt.id,
              actor: attempt.actor,
              epoch: attempt.epoch,
              expires: attempt.expires,
              reserved: attempt.reserved,
            },
          }
        : {}),
    },
  };
}
