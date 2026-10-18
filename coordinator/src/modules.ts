// Module projects (docs/mathdoc.md §4.13): every document node with a Lean block is a
// whole Lean file, compiled as written. The root node holds the statement; the Lean files it
// imports or depends on are the project's shared modules. A contribution is a set of files
// (`mdc prove` / `mdc split` on a pulled directory) that LeanGround compiles in a sandbox and
// checks at the constant level; it never sees a contribution's files rewritten.
//
// Nothing here decides whether a proof is right. File lists, statement text for drafts and the
// edges of written-back nodes are read from the source; when they are off, LeanGround's
// compiler and kernel refuse the submission or the reader sees an odd edge, nothing more.
import { createHash } from "node:crypto";
import type { z } from "zod";
import {
  attached,
  Fault,
  MODULE_PATH,
  requireThat,
  type Board,
  type Declaration,
  type Run,
  type SharedModule,
} from "./domain.js";
import { writebackContractSchema } from "./contracts.js";
import { draftWork, type GoalDraft } from "./goal-draft.js";
import { moduleRoot, nodeRevision, type Node } from "./docs/model.js";
import type { Snapshot } from "./docs/graph.js";
import type { NodeBinding, ProofRequest } from "./proof.js";
import { batchId, Conflict, derivedUuid, type Writeback } from "./writeback.js";

export const leanBlock = (n: Node) =>
  n.blocks.find((b) => b.srctype === "lean");
const sha = (text: string) =>
  createHash("sha256").update(text, "utf8").digest("hex");

/**
 * The Lean module a node's file is. A written-back or imported file keeps the name it was
 * compiled under (`lean_module`, `import_module`); any other node is its own document module.
 */
export function moduleOfNode(n: Node): string {
  const m = leanBlock(n)?.metadata;
  return m?.lean_module || m?.import_module || n.module;
}
/** `A.B.C` → `A/B/C.lean`; undefined for a name LeanGround takes as no path (e.g. `«a b»`). */
export function modulePath(module: string): string | undefined {
  const path = `${module.split(".").join("/")}.lean`;
  return MODULE_PATH.test(path) ? path : undefined;
}
export const pathModule = (path: string) =>
  path.slice(0, -".lean".length).split("/").join(".");

/**
 * The `import`s of a file's header, up to its first other command: LeanGround's rule
 * (`module_facts.header_imports`), so both sides read the same file list.
 */
export function headerImports(text: string): string[] {
  const out: string[] = [];
  for (const line of text.replace(/\/-[\s\S]*?-\//g, " ").split("\n")) {
    const s = line.split("--", 1)[0].trim();
    if (!s || s === "module" || s === "prelude") continue;
    const m = /^(?:(?:public|private|meta)\s+)*import\s+(?:all\s+)?(.+)$/.exec(
      s,
    );
    if (!m) break;
    out.push(...m[1].split(/\s+/).filter(Boolean));
  }
  return out;
}

/** A project file held by a document node. */
export interface ProjectFile {
  node: string;
  title: string;
  module: string;
  path: string;
  content: string;
}
/**
 * The root's statement file and the shared module files it needs, dependencies first: Lean
 * nodes reached from the root by dependency edges or by `import`s of their header. Files a
 * writeback created for a contribution are not project modules and are never followed.
 * `problems` lists nodes that cannot be files, with why.
 */
export function projectFiles(nodes: Map<string, Node>, root: string) {
  const byModule = new Map<string, string>();
  for (const n of nodes.values())
    if (leanBlock(n) && !leanBlock(n)!.metadata.contribution)
      byModule.set(moduleOfNode(n), n.fnode);
  const order: string[] = [];
  const seen = new Set<string>();
  const visit = (id: string) => {
    if (seen.has(id)) return;
    seen.add(id);
    const n = nodes.get(id);
    const lean = n && leanBlock(n);
    if (!lean || lean.metadata.contribution) return;
    const imported = headerImports(lean.content)
      .map((m) => byModule.get(m))
      .filter((x): x is string => x !== undefined);
    for (const d of [...n.depens, ...imported]) visit(d);
    order.push(id);
  };
  visit(root);
  const files: ProjectFile[] = [];
  const problems = new Map<string, { reason: string; details: string }>();
  const owner = new Map<string, string>();
  for (const id of order) {
    const n = nodes.get(id)!;
    const module = moduleOfNode(n);
    const path = modulePath(module);
    if (!path) {
      problems.set(id, {
        reason: "invalid_module_name",
        details: `${module} is not a module name LeanGround takes as a path (A.B, identifiers only)`,
      });
      continue;
    }
    if (owner.has(path)) {
      problems.set(id, {
        reason: "module_name_conflict",
        details: `node ${nodes.get(owner.get(path)!)!.title} is the same module ${module}`,
      });
      continue;
    }
    owner.set(path, id);
    files.push({
      node: id,
      title: n.title,
      module,
      path,
      content: leanBlock(n)!.content,
    });
  }
  return {
    root: files.find((f) => f.node === root),
    shared: files.filter((f) => f.node !== root),
    problems,
  };
}

/** The name and signature between the name and `:=`, cut at its top-level `:`. */
function signature(text: string): { binders: string; type: string } {
  const open = "([{⦃⟨";
  const close = ")]}⦄⟩";
  let depth = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (open.includes(c)) depth++;
    else if (close.includes(c)) depth--;
    else if (c === ":" && depth === 0 && text[i + 1] !== "=")
      return {
        binders: text.slice(0, i).trim(),
        type: text.slice(i + 1).trim(),
      };
  }
  return { binders: "", type: text.trim() };
}
/**
 * For display only: how the files state `constant`, as `theorem constant binders : type`, from
 * the declaration named by its last component. Undefined when no such text is found.
 */
export function statementOf(
  files: Record<string, string>,
  constant: string,
): Declaration | undefined {
  const last = constant.split(".").at(-1)!;
  const escaped = last.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const head = new RegExp(
    `^(?:@\\[[^\\]]*\\]\\s*)?(?:(?:private|protected|nonrec)\\s+)*(?:theorem|lemma)\\s+(?:[\\w.']+\\.)?${escaped}(?=[\\s:({\\[⦃])`,
    "m",
  );
  for (const path of Object.keys(files).sort()) {
    const text = files[path];
    const m = head.exec(text);
    if (!m) continue;
    const rest = text.slice(m.index + m[0].length);
    const end = rest.indexOf(":=");
    if (end < 0) continue;
    const s = signature(rest.slice(0, end).replace(/\s+/g, " "));
    if (!s.type) continue;
    return { name: constant, ...s };
  }
  return undefined;
}
/** The file declaring `constant`, else a file no other file of the set imports. */
export function rootFile(
  files: Record<string, string>,
  constant: string,
): string {
  const paths = Object.keys(files).sort();
  const named = statementOf(files, constant);
  if (named)
    for (const p of paths)
      if (statementOf({ [p]: files[p] }, constant)) return p;
  const imported = new Set(
    paths.flatMap((p) => headerImports(files[p])).map((m) => modulePath(m)),
  );
  return paths.find((p) => !imported.has(p)) ?? paths[0];
}

/** A certificate's file set (`lean-modules-v1` sources are the files as canonical JSON). */
export function certificateFiles(
  source: unknown,
): Record<string, string> | undefined {
  if (typeof source !== "string") return undefined;
  try {
    const files = JSON.parse(source);
    if (
      files &&
      typeof files === "object" &&
      Object.entries(files).every(
        ([p, t]) => MODULE_PATH.test(p) && typeof t === "string",
      )
    )
      return files as Record<string, string>;
  } catch {
    // not a file set
  }
  return undefined;
}

/**
 * The files stating a goal: the root node's file (at its module path), the decomposition with
 * its placeholder, or a negation: the refuted goal's files with `¬` stated at the end of one.
 */
export interface ModuleStatementFiles {
  const: string;
  from: "node" | "decomposition" | "negation";
  node?: string;
  certificate?: string;
  /** A negation: the goal it refutes and the file whose end states it. */
  negates?: { goal: string; path: string };
  files: Record<string, string>;
}
export interface ModuleDraft {
  format: "mdc-module-draft/1";
  project: string;
  revision: number;
  environment_id: string;
  goal: string;
  status: string;
  branch: string | null;
  base: { root_module: string; lean_version: string };
  statement: ModuleStatementFiles;
  /** Shared modules the statement uses, dependencies first; proofs import them, never change them. */
  shared: { id: string; modules: string[]; files: Record<string, string> }[];
  notes: GoalDraft["notes"];
  task: GoalDraft["task"];
}

/** Reads a statement source needs: branch documents and certificates (with their source). */
export interface StatementSources {
  board: Board;
  branch?: string;
  snapshot: (branch: string) => Promise<Snapshot>;
  certificate: (id: string) => Promise<any>;
}
/** Negations of negations are followed this deep at most. */
const NEGATION_DEPTH = 8;

/**
 * The files stating a module goal (docs/mathdoc.md §4.13). The root's file is read from a
 * branch whose text is still the converted one; a negation is rebuilt from the goal it refutes.
 */
export async function moduleStatement(
  input: StatementSources,
  goal: string,
  depth = 0,
): Promise<ModuleStatementFiles> {
  const b = input.board;
  requireThat(Object.hasOwn(b.goals, goal), "unknown_goal", 404);
  const g = b.goals[goal];
  requireThat(g.identity && g.module, "goal_unresolved");
  const m = g.module;
  if (m.negates) {
    requireThat(depth < NEGATION_DEPTH, "statement_unavailable");
    const target = await moduleStatement(input, m.negates.goal, depth + 1);
    const files = statementLayout(target);
    requireThat(Object.hasOwn(files, m.negates.path), "statement_unavailable");
    return {
      const: m.const,
      from: "negation",
      negates: m.negates,
      files: {
        ...files,
        [m.negates.path]:
          files[m.negates.path] + negationText(target.const, m.const),
      },
    };
  }
  if (m.certificate) {
    const cert = await input.certificate(m.certificate);
    const files = certificateFiles(cert.source);
    requireThat(files, "statement_unavailable");
    return {
      const: m.const,
      from: "decomposition",
      certificate: m.certificate,
      files,
    };
  }
  requireThat(m.node && m.path, "statement_unavailable");
  // The branch asked for first, then any attached branch: the root is the same on all of them.
  const names = [
    ...(input.branch !== undefined ? [input.branch] : []),
    ...Object.keys(b.branches ?? {}).sort(),
  ];
  for (const name of new Set(names)) {
    if (!b.branches || !Object.hasOwn(b.branches, name)) continue;
    const bound = attached(b, name).nodes[m.node];
    if (!bound) continue;
    const node = (await input.snapshot(name)).nodes.get(m.node);
    const lean = node && leanBlock(node);
    if (lean && sha(lean.content) === bound.lean_content_sha256)
      return {
        const: m.const,
        from: "node",
        node: m.node,
        files: { [m.path]: lean.content },
      };
  }
  throw new Fault("statement_changed", 409);
}

/**
 * The statement's files as a goal directory has them: the root's file becomes Work/Main.lean
 * (a proof restates the theorem in its own module, never under the statement's module name);
 * other statements keep their paths. `mdc/module-dir.ts` `startingFiles` does the same.
 */
export function statementLayout(
  s: ModuleStatementFiles,
): Record<string, string> {
  return s.from === "node"
    ? { "Work/Main.lean": Object.values(s.files)[0] }
    : { ...s.files };
}

/** A constant's name as source writes it: `_private.M.0.x` is `x` inside module M. */
export function userName(constant: string): string {
  const m = /^_private\.(?:.+?)\.0\.(.+)$/.exec(constant);
  return m ? m[1] : constant;
}
/** A name in Lean syntax: components that are not identifiers go in «». */
function leanName(name: string): string {
  return name
    .split(".")
    .map((c) =>
      /^[\p{L}_][\p{L}\p{N}_'!?]*$/u.test(c) && c !== "_root_" ? c : `«${c}»`,
    )
    .join(".");
}
/** The namespace negation theorems are declared in, out of the way of the statement's names. */
export const NEGATION_NAMESPACE = "MDCRefute";
/** The negation of `constant`: `MDCRefute.not_<its last component>`. */
export function negationName(constant: string): string {
  return `${NEGATION_NAMESPACE}.not_${userName(constant).split(".").at(-1)}`;
}
/**
 * `¬ constant`, stated after the theorem it negates in the same file: by full names from the
 * root namespace, so neither an open namespace nor a shadowing name changes it, and a private
 * theorem is still in reach. `type_of%` takes the statement as elaborated; its universe
 * parameters become the negation's.
 */
export function negationText(constant: string, negation: string): string {
  return `\ntheorem _root_.${leanName(negation)} : ¬ type_of% @_root_.${leanName(userName(constant))} := sorry\n`;
}
/**
 * Where `¬ goal` goes and its files: the end of the file declaring the statement constant (for
 * the root, Work/Main.lean).
 */
export function negationOf(stated: ModuleStatementFiles): {
  const: string;
  path: string;
  files: Record<string, string>;
} {
  const files = statementLayout(stated);
  const path =
    stated.from === "negation"
      ? stated.negates!.path
      : rootFile(files, userName(stated.const));
  const negation = negationName(stated.const);
  return {
    const: negation,
    path,
    files: {
      ...files,
      [path]: files[path] + negationText(stated.const, negation),
    },
  };
}
/** How a negation reads on the board, from the declaration it negates (display only). */
export function negatedDeclaration(
  name: string,
  d: Declaration | undefined,
  constant: string,
): Declaration {
  const type = !d
    ? `¬ ${userName(constant)}`
    : d.binders
      ? `¬ (∀ ${d.binders}, ${d.type})`
      : `¬ (${d.type})`;
  return { name, binders: "", type };
}

/**
 * The material of a module goal's directory (`mdc pull`): its statement files and the shared
 * modules they use.
 */
export async function moduleDraft(input: {
  board: Board;
  goal: string;
  branch?: string;
  snapshot: (branch: string) => Promise<Snapshot>;
  certificate: (id: string) => Promise<any>;
  shared: (id: string) => Promise<any>;
}): Promise<ModuleDraft> {
  const b = input.board;
  requireThat(b.project && b.mode === "modules", "not_a_module_project", 400);
  requireThat(Object.hasOwn(b.goals, input.goal), "unknown_goal", 404);
  const g = b.goals[input.goal];
  requireThat(g.identity && g.module, "goal_unresolved");
  const m = g.module;
  const statement = await moduleStatement(input, input.goal);
  // Shared modules with their dependencies, dependencies first.
  const shared: ModuleDraft["shared"] = [];
  const seen = new Set<string>();
  const visit = async (id: string) => {
    if (seen.has(id)) return;
    seen.add(id);
    const record = await input.shared(id);
    for (const dep of [...(record.shared ?? [])].sort()) await visit(dep);
    const files = certificateFiles(record.source);
    requireThat(files, "shared_module_unavailable");
    shared.push({
      id,
      modules: (record.modules ?? []).map((x: { module: string }) => x.module),
      files,
    });
  };
  for (const id of [...m.shared].sort()) await visit(id);
  return {
    format: "mdc-module-draft/1",
    project: b.id,
    revision: b.revision,
    environment_id: b.project.environment_id,
    goal: input.goal,
    status: g.status,
    branch:
      input.branch !== undefined &&
      b.branches &&
      Object.hasOwn(b.branches, input.branch)
        ? input.branch
        : null,
    base: {
      root_module: String(b.base.root_module ?? ""),
      lean_version: String(b.base.lean_version ?? ""),
    },
    statement,
    shared,
    ...draftWork(b, input.goal),
  };
}

export interface ModuleCertificateRecord {
  id: string;
  goal: string;
  premises: string[];
  bindings: { name: string; role: string; goal: string }[];
  source: string;
  shared?: string[];
}
export interface SharedRecord {
  id: string;
  modules: { module: string }[];
  shared?: string[];
  source: string;
}

/**
 * The writeback of a certified module route (docs/mathdoc.md §4.13). Every file of
 * every certificate on the route becomes a node whose Lean block is the file as certified;
 * edges follow its imports inside the certificate and into shared modules, and lead from a
 * decomposition's root file to its providers' root files. Shared modules registered by a
 * decomposition get nodes too. The root node keeps its text (it is the statement) and records
 * the final certificate. Files written by an earlier batch are reused, so writing the same
 * certificate twice never duplicates its nodes. Any changed statement file aborts the batch.
 */
export function buildModuleBatch(input: {
  board: Board;
  request: ProofRequest;
  nodes: Record<string, NodeBinding>;
  run: Run;
  snapshot: Snapshot;
  certificates: Map<string, ModuleCertificateRecord>;
  shared: Map<string, SharedRecord>;
  label: (goal: string) => string;
}): { changes: Node[]; writeback: Writeback } {
  const { request, run, snapshot: s } = input;
  const batch = batchId(run);
  const changes = new Map<string, Node>();
  const created: string[] = [];
  const bindingsOfCreated: NodeBinding[] = [];
  const operations: z.infer<typeof writebackContractSchema>["operations"] = [];
  // The statement files the route was converted from must be unchanged.
  for (const n of Object.values(input.nodes)) {
    if (n.file) continue;
    const current = s.nodes.get(n.node);
    const lean = current && leanBlock(current);
    if (!lean)
      throw new Conflict(
        "document_conflict",
        `node ${n.title} was deleted or lost its Lean block`,
      );
    if (sha(lean.content) !== n.lean_content_sha256)
      throw new Conflict(
        "document_conflict",
        `node ${n.title} changed after it was converted; the statement may no longer be the same`,
      );
  }
  const moduleRootName = moduleRoot(s.project);
  const fresh = (key: string, title: string): Node => {
    const fnode = derivedUuid(batch, key);
    return {
      fnode,
      title,
      module: `${moduleRootName}.N_${fnode.replaceAll("-", "")}`,
      depens: [],
      blocks: [],
    };
  };
  const existing = (match: (m: Record<string, string>) => boolean) => {
    for (const n of s.nodes.values()) {
      const lean = leanBlock(n);
      if (lean && match(lean.metadata)) return structuredClone(n);
    }
    return undefined;
  };
  const write = (
    n: Node,
    content: string,
    metadata: Record<string, string>,
  ) => {
    const prior = leanBlock(n);
    const block = {
      srctype: "lean",
      content,
      metadata: { ...(prior?.metadata ?? {}), ...metadata },
    };
    n.blocks = prior
      ? n.blocks.map((x) => (x.srctype === "lean" ? block : x))
      : [...n.blocks, block];
  };

  // Project modules by name: the root's shared files, then shared modules a decomposition
  // registered (from an earlier batch, or new nodes now).
  const byModule = new Map<string, string>();
  for (const n of Object.values(input.nodes))
    if (n.role === "definition" && s.nodes.has(n.node))
      byModule.set(moduleOfNode(s.nodes.get(n.node)!), n.node);
  const pendingEdges: [Node, string[]][] = [];
  const known = new Set(
    Object.values(input.board.shared_modules ?? {})
      .filter((x) => x.nodes?.length)
      .map((x) => x.id),
  );
  for (const record of input.shared.values()) {
    if (known.has(record.id)) continue;
    const files = certificateFiles(record.source);
    if (!files)
      throw new Conflict(
        "shared_module_unavailable",
        `shared module ${record.id}`,
      );
    for (const path of Object.keys(files).sort()) {
      const module = pathModule(path);
      const prior = existing(
        (m) => m.shared_module_id === record.id && m.module_path === path,
      );
      const n = prior ?? fresh(`shared:${record.id}:${path}`, module);
      write(n, files[path], {
        lean_module: module,
        module_path: path,
        shared_module_id: record.id,
        coordination_operation: batch,
      });
      byModule.set(module, n.fnode);
      pendingEdges.push([n, headerImports(files[path])]);
      changes.set(n.fnode, n);
      if (!prior) {
        created.push(n.fnode);
        bindingsOfCreated.push({
          node: n.fnode,
          title: n.title,
          role: "definition",
          lean_content_sha256: sha(files[path]),
          definition_ids: [record.id],
          state: "registered",
          file: { path, shared: record.id },
        });
        operations.push({
          operation_id: `${batch}:${n.fnode}`,
          kind: "create_definition_node",
          node_id: n.fnode,
          source_sha256: sha(files[path]),
          definition_id: record.id,
          preserve_public_declaration_name: true,
        });
      }
    }
  }

  // Contribution files, and each certificate's root file.
  const rootOf = new Map<string, string>(); // certificate → node of its root file
  for (const step of run.route.steps) {
    const cert = input.certificates.get(step.certificate_id);
    if (!cert) throw new Conflict("unknown_certificate", step.certificate_id);
    const files = certificateFiles(cert.source);
    if (!files)
      throw new Conflict(
        "not_a_module_certificate",
        `certificate ${cert.id} has no file set`,
      );
    const root = cert.bindings.find((x) => x.role === "conclusion")?.name ?? "";
    const main = rootFile(files, root);
    const local = new Map<string, string>();
    const label = input.label(cert.goal);
    const made: [Node, string][] = [];
    for (const path of Object.keys(files).sort()) {
      const prior = existing(
        (m) => m.contribution === cert.id && m.module_path === path,
      );
      const n = prior ?? fresh(`file:${cert.id}:${path}`, `${label} — ${path}`);
      local.set(pathModule(path), n.fnode);
      if (path === main) rootOf.set(cert.id, n.fnode);
      made.push([n, path]);
      write(n, files[path], {
        lean_module: pathModule(path),
        module_path: path,
        contribution: cert.id,
        certification_id: cert.id,
        certified_lean_sha256: sha(files[path]),
        coordination_operation: batch,
      });
      changes.set(n.fnode, n);
      if (!prior) {
        created.push(n.fnode);
        bindingsOfCreated.push({
          node: n.fnode,
          title: n.title,
          role: "theorem",
          lean_content_sha256: sha(files[path]),
          goal: cert.goal,
          state: "certified",
          certificate: cert.id,
          file: { path, certificate: cert.id, root: path === main },
        });
      }
      operations.push({
        operation_id: `${batch}:${n.fnode}`,
        kind: prior ? "update_lean_block" : "create_goal_node",
        node_id: n.fnode,
        ...(prior
          ? { expected_node_revision: nodeRevision(s.nodes.get(n.fnode)!) }
          : {}),
        source_sha256: sha(files[path]),
        certification_id: cert.id,
        preserve_public_declaration_name: true,
      });
    }
    for (const [n, path] of made)
      pendingEdges.push([
        n,
        headerImports(files[path]).map((m) =>
          local.has(m) ? `\0${local.get(m)}` : m,
        ),
      ]);
  }
  // Import edges: `\0id` is a file of the same contribution, else a project module by name.
  for (const [n, imports] of pendingEdges) {
    const deps = new Set(n.depens);
    for (const m of imports) {
      const target = m.startsWith("\0") ? m.slice(1) : byModule.get(m);
      if (target && target !== n.fnode) deps.add(target);
    }
    n.depens = [...deps];
  }
  // A decomposition depends on the proofs of its premises.
  for (const step of run.route.steps) {
    const from = changes.get(rootOf.get(step.certificate_id)!)!;
    const deps = new Set(from.depens);
    for (const provider of Object.values(step.providers)) {
      const to = rootOf.get(provider);
      if (to && to !== from.fnode) deps.add(to);
    }
    from.depens = [...deps];
  }
  // The root keeps its statement and records the final certificate.
  const rootNode = s.nodes.get(request.node);
  const rootStep = run.route.steps.find(
    (x) => x.certificate_id === run.route.root,
  );
  if (!rootNode || !rootStep)
    throw new Conflict(
      "root_not_in_route",
      "the certified route does not end at the bound root node",
    );
  const root = structuredClone(rootNode);
  const content = leanBlock(root)!.content;
  root.depens = [
    ...new Set([...root.depens, rootOf.get(rootStep.certificate_id)!]),
  ];
  write(root, content, {
    certification_id: run.certification_id!,
    certified_lean_sha256: sha(content),
    coordination_operation: batch,
  });
  changes.set(root.fnode, root);
  operations.push({
    operation_id: `${batch}:${root.fnode}`,
    kind: "update_lean_block",
    node_id: root.fnode,
    expected_node_revision: nodeRevision(rootNode),
    source_sha256: sha(content),
    certification_id: run.certification_id!,
    preserve_public_declaration_name: true,
  });
  const contract = writebackContractSchema.parse({
    schema_version: "mathdoc.writeback-batch.v1",
    batch_id: batch,
    replay_key: run.id,
    review_branch: {
      database: request.database,
      branch: request.branch,
      data_version: s.version,
    },
    source_proof_request_id: request.identity,
    operations,
    conflict_resolution: "abort_entire_batch",
    certification_visibility: "after_full_batch_commit",
  });
  return {
    changes: [...changes.values()],
    writeback: {
      batch_id: batch,
      run: run.id,
      status: "committed",
      contract,
      created,
      updated: [...changes.keys()].filter((id) => !created.includes(id)),
      bindings: bindingsOfCreated,
    },
  };
}

/** Shared modules the route's certificates use that no document node holds yet. */
export function routeSharedModules(
  b: Board,
  certificates: Map<string, ModuleCertificateRecord>,
): string[] {
  const held = (id: string) => (b.shared_modules?.[id]?.nodes?.length ?? 0) > 0;
  return [
    ...new Set(
      [...certificates.values()]
        .flatMap((c) => c.shared ?? [])
        .filter((id) => !held(id)),
    ),
  ].sort();
}

/** Record a shared module on the board (an ID registered twice keeps its first record). */
export function addSharedModule(b: Board, m: SharedModule) {
  b.shared_modules ??= {};
  if (!Object.hasOwn(b.shared_modules, m.id)) b.shared_modules[m.id] = m;
}
