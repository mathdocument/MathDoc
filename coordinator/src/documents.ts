import { randomUUID } from "node:crypto";
import { z } from "zod";
import { type NodeRef, requireThat } from "./domain.js";
import { RemoteError } from "./remote.js";
export const blockSchema = z
  .object({
    srctype: z.enum(["text", "latex", "lean", "rocq"]),
    content: z.string().max(2000000),
    metadata: z.record(z.string()).default({}),
  })
  .strict();
export const nodeSchema = z
  .object({
    fnode: z.string().uuid(),
    title: z.string().trim().min(1).max(1000),
    module: z
      .string()
      .regex(/^[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)*$/),
    depens: z.array(z.string().uuid()),
    blocks: z.array(blockSchema),
  })
  .strict();
export type DocNode = z.infer<typeof nodeSchema>;
export interface Snapshot {
  version: string;
  nodes: DocNode[];
}
export class Documents {
  constructor(
    private url?: string,
    private user = "admin",
    private password?: string,
  ) {}
  private path(ref: Pick<NodeRef, "database" | "branch">) {
    requireThat(
      /^[\w-]+$/.test(ref.database) && /^[\w-]+$/.test(ref.branch),
      "invalid_document_path",
    );
    return `admin/${ref.database}/local/branch/${ref.branch}`;
  }
  private async request(
    path: string,
    method = "GET",
    body?: unknown,
    version?: string,
  ) {
    requireThat(this.url && this.password, "documents_not_configured", 503);
    let r: Response;
    try {
      r = await fetch(`${this.url.replace(/\/$/, "")}/api/${path}`, {
        method,
        headers: {
          authorization: `Basic ${Buffer.from(`${this.user}:${this.password}`).toString("base64")}`,
          "content-type": "application/json",
          ...(version ? { "TerminusDB-Data-Version": version } : {}),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(30000),
        redirect: "error",
      });
    } catch {
      throw new RemoteError("documents_unavailable", true, 0);
    }
    if (!r.ok) {
      const detail = await r.text();
      if (
        r.status === 409 ||
        detail.includes("DataVersion") ||
        detail.includes("data_version")
      )
        throw new RemoteError("document_revision_conflict", false, 409);
      throw new RemoteError(`documents_${r.status}`, r.status >= 500, r.status);
    }
    return r;
  }
  async load(ref: Pick<NodeRef, "database" | "branch">): Promise<Snapshot> {
    const r = await this.request(
      `document/${this.path(ref)}?as_list=true&unfold=false`,
    );
    const version = r.headers.get("TerminusDB-Data-Version");
    requireThat(version, "missing_document_version", 502);
    const values = z.array(z.record(z.unknown())).parse(await r.json());
    const nodes = values
      .filter((d) => d["@type"] === "Node")
      .map((d) =>
        nodeSchema.parse({
          fnode: d.fnode,
          title: d.title,
          module: d.module,
          depens: z
            .array(z.string())
            .parse(d.depens ?? [])
            .map((x) => x.split("/").at(-1)),
          blocks: JSON.parse(z.string().parse(d.blocks)),
        }),
      );
    return { version, nodes };
  }
  async put(
    ref: Pick<NodeRef, "database" | "branch">,
    node: DocNode,
    version: string,
    actor: string,
    certifiedWrite = false,
  ): Promise<string> {
    const snapshot = await this.load(ref);
    requireThat(snapshot.version === version, "document_revision_conflict");
    if (!certifiedWrite)
      for (const block of node.blocks) {
        delete block.metadata.certification_id;
        delete block.metadata.coordination_operation;
      }
    const old = snapshot.nodes.find((n) => n.fnode === node.fnode);
    if (old)
      requireThat(old.module === node.module, "module_identity_immutable");
    requireThat(
      new Set(node.blocks.map((b) => b.srctype)).size === node.blocks.length,
      "duplicate_block",
    );
    requireThat(
      new Set(node.depens).size === node.depens.length,
      "duplicate_dependency",
    );
    const nodes = new Map(snapshot.nodes.map((n) => [n.fnode, n]));
    nodes.set(node.fnode, node);
    const visiting = new Set<string>();
    const done = new Set<string>();
    function walk(id: string) {
      requireThat(nodes.has(id), "unknown_dependency");
      requireThat(!visiting.has(id), "dependency_cycle");
      if (done.has(id)) return;
      visiting.add(id);
      for (const dep of nodes.get(id)!.depens) walk(dep);
      visiting.delete(id);
      done.add(id);
    }
    for (const id of nodes.keys()) walk(id);
    const r = await this.request(
      `document/${this.path(ref)}?create=true&author=${encodeURIComponent(actor)}&message=Update%20MathDoc%20node`,
      "PUT",
      [
        {
          "@id": `Node/${node.fnode}`,
          "@type": "Node",
          ...node,
          depens: node.depens.map((id) => `Node/${id}`),
          blocks: JSON.stringify(node.blocks),
        },
      ],
      version,
    );
    const next = r.headers.get("TerminusDB-Data-Version");
    requireThat(next, "missing_document_version", 502);
    return next;
  }
  async history(ref: Pick<NodeRef, "database" | "branch">) {
    return (await this.request(`log/${this.path(ref)}?count=50`)).json();
  }
  async branch(ref: Pick<NodeRef, "database" | "branch">, name: string) {
    requireThat(/^[\w-]+$/.test(name), "invalid_branch");
    return (
      await this.request(
        `branch/admin/${ref.database}/local/branch/${name}`,
        "POST",
        { origin: this.path(ref) },
      )
    ).json();
  }
  async accept(
    ref: NodeRef,
    source: string,
    cert: string,
    operation: string,
    actor: string,
  ): Promise<string> {
    const s = await this.load(ref);
    const node = s.nodes.find((n) => n.fnode === ref.node);
    requireThat(node, "document_not_found", 404);
    const prior = node.blocks.find((b) => b.srctype === "lean");
    // Recover a completed write whose response was lost, even if unrelated branch
    // edits followed. Do not apply twice or overwrite those edits.
    if (
      prior?.metadata.coordination_operation === operation &&
      prior.metadata.certification_id === cert &&
      prior.content === source
    )
      return s.version;
    requireThat(s.version === ref.revision, "document_revision_conflict");
    const block = {
      srctype: "lean" as const,
      content: source,
      metadata: {
        ...prior?.metadata,
        certification_id: cert,
        coordination_operation: operation,
      },
    };
    node.blocks = node.blocks.filter((b) => b.srctype !== "lean").concat(block);
    return this.put(ref, node, s.version, actor, true);
  }
  static empty(title: string): DocNode {
    const fnode = randomUUID();
    return {
      fnode,
      title,
      module: `Lib.N_${fnode.replaceAll("-", "")}`,
      depens: [],
      blocks: [],
    };
  }
}
