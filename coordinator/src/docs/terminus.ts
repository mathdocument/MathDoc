// TerminusDB access for versioned MathDoc documents (Terminus/Database in src/store.rs).
// Every write carries TerminusDB-Data-Version, so a concurrent commit is a conflict,
// never a silent overwrite.
import { STATUS_CODES } from "node:http";
import { DocError, validateName } from "./names.js";
import {
  DEFAULT_LATEX_PROJECT,
  DEFAULT_LEAN_PROJECT,
  type LatexProject,
  type LeanProject,
  type Node,
  latexProjectJson,
  leanProjectJson,
  nodeDocument,
  nodeFromDocument,
  parseLatexProject,
  parseLeanProject,
  validateLatexProject,
  validateLeanProject,
} from "./model.js";
import { Snapshot } from "./graph.js";

export interface TerminusConfig {
  url: string;
  user: string;
  password: string;
}

export class Terminus {
  constructor(readonly config: TerminusConfig) {
    const parsed = new URL(config.url);
    if (!["http:", "https:"].includes(parsed.protocol))
      throw new DocError("invalid database URL");
  }

  async request(
    method: string,
    path: string,
    query: Record<string, string> = {},
    body?: unknown,
    version?: string,
  ): Promise<Response> {
    const url = new URL(`${this.config.url.replace(/\/$/, "")}/api/${path}`);
    for (const [k, v] of Object.entries(query)) url.searchParams.set(k, v);
    let response: Response;
    try {
      response = await fetch(url, {
        method,
        headers: {
          authorization: `Basic ${Buffer.from(`${this.config.user}:${this.config.password}`).toString("base64")}`,
          ...(body === undefined ? {} : { "content-type": "application/json" }),
          ...(version ? { "TerminusDB-Data-Version": version } : {}),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(120000),
        redirect: "error",
      });
    } catch {
      throw new DocError("connecting to TerminusDB", 502);
    }
    if (!response.ok) {
      const text = await response.text().catch(() => "");
      if (text.includes("DataVersion") || text.includes("data_version"))
        throw new DocError("database revision conflict; reload and retry", 409);
      const status =
        `${response.status} ${STATUS_CODES[response.status] ?? ""}`.trim();
      throw new DocError(
        `TerminusDB ${status}: ${[...text].slice(0, 1500).join("")}`,
        502,
      );
    }
    return response;
  }

  /** Every branch of every MathDoc-labelled database, sorted by (database, branch). */
  async projects(): Promise<{ database: string; branch: string }[]> {
    const inventory = (await (
      await this.request("GET", "db", { verbose: "true", branches: "true" })
    ).json()) as { path: string; label?: string | null; branches: string[] }[];
    const out: { database: string; branch: string }[] = [];
    for (const info of inventory) {
      if (info.label !== "MathDoc" || !info.path.startsWith("admin/")) continue;
      const database = info.path.slice("admin/".length);
      for (const branch of info.branches) {
        validateName(database);
        validateName(branch);
        out.push({ database, branch });
      }
    }
    return out.sort((a, b) =>
      a.database === b.database
        ? a.branch < b.branch
          ? -1
          : a.branch > b.branch
            ? 1
            : 0
        : a.database < b.database
          ? -1
          : 1,
    );
  }

  database(database: string, branch: string) {
    return new Database(this, database, branch);
  }
}

function dataVersion(response: Response): string {
  const version = response.headers.get("TerminusDB-Data-Version");
  if (!version) throw new DocError("TerminusDB omitted data version", 502);
  return version;
}

export class Database {
  constructor(
    readonly server: Terminus,
    readonly database: string,
    readonly branch: string,
  ) {
    validateName(database);
    validateName(branch);
  }
  get path() {
    return `admin/${this.database}/local/branch/${this.branch}`;
  }

  async initialize(author: string) {
    await this.server.request(
      "POST",
      `db/admin/${this.database}`,
      {},
      {
        label: "MathDoc",
        comment: "Versioned MathDoc nodes",
      },
    );
    const schema = [
      {
        "@type": "Class",
        "@id": "Node",
        "@key": { "@type": "Lexical", "@fields": ["fnode"] },
        fnode: "xsd:string",
        title: "xsd:string",
        module: "xsd:string",
        blocks: "xsd:string",
        depens: { "@type": "Set", "@class": "Node" },
      },
      {
        "@type": "Class",
        "@id": "Project",
        "@key": { "@type": "Lexical", "@fields": ["name"] },
        name: "xsd:string",
        config: "xsd:string",
      },
    ];
    await this.server.request(
      "POST",
      `document/${this.path}`,
      {
        graph_type: "schema",
        author,
        message: "Initialize MathDoc schema",
      },
      schema,
    );
    await this.server.request(
      "POST",
      `document/${this.path}`,
      {
        author,
        message: "Initialize Lean project",
      },
      {
        "@type": "Project",
        name: "lean",
        config: leanProjectJson(DEFAULT_LEAN_PROJECT),
      },
    );
  }

  async version(): Promise<string> {
    return dataVersion(
      await this.server.request("GET", `document/${this.path}`, {
        count: "0",
        as_list: "true",
      }),
    );
  }

  async load(): Promise<Snapshot> {
    const response = await this.server.request("GET", `document/${this.path}`, {
      as_list: "true",
      unfold: "false",
    });
    const version = dataVersion(response);
    const docs = (await response.json()) as Record<string, unknown>[];
    const nodes: Node[] = [];
    let project: LeanProject | null = null;
    let latex: LatexProject = { ...DEFAULT_LATEX_PROJECT };
    for (const doc of docs) {
      if (doc["@type"] === "Node") nodes.push(nodeFromDocument(doc));
      else if (
        doc["@type"] === "Project" &&
        (doc.name === "lean" || doc.name === "latex")
      ) {
        if (typeof doc.config !== "string")
          throw new DocError(
            doc.name === "lean"
              ? "invalid project config"
              : "invalid LaTeX project config",
          );
        const config = JSON.parse(doc.config);
        if (doc.name === "lean") project = parseLeanProject(config);
        else {
          latex = parseLatexProject(config);
          validateLatexProject(latex);
        }
      }
    }
    if (!project) throw new DocError("database has no Lean project");
    // The stored Lake configuration was validated by whichever backend wrote it. Loading
    // trusts it: the fail-closed TOML subset must never make an existing branch unreadable.
    const snapshot = new Snapshot(version, [], project, latex);
    snapshot.validateChanges(nodes);
    for (const n of nodes) snapshot.nodes.set(n.fnode, n);
    snapshot.recomputeGraph();
    return snapshot;
  }

  async putBundle(
    nodes: Node[],
    project: LeanProject | null,
    latex: LatexProject | null,
    version: string,
    message: string,
    author: string,
  ): Promise<string> {
    const documents: unknown[] = nodes.map(nodeDocument);
    if (project)
      documents.push({
        "@id": "Project/lean",
        "@type": "Project",
        name: "lean",
        config: leanProjectJson(project),
      });
    if (latex) {
      validateLatexProject(latex);
      documents.push({
        "@id": "Project/latex",
        "@type": "Project",
        name: "latex",
        config: latexProjectJson(latex),
      });
    }
    return dataVersion(
      await this.server.request(
        "PUT",
        `document/${this.path}`,
        { create: "true", author, message },
        documents,
        version,
      ),
    );
  }

  async putProject(
    project: LeanProject,
    version: string,
    author: string,
  ): Promise<string> {
    validateLeanProject(project);
    return dataVersion(
      await this.server.request(
        "PUT",
        `document/${this.path}`,
        { author, message: "Update Lean environment" },
        {
          "@id": "Project/lean",
          "@type": "Project",
          name: "lean",
          config: leanProjectJson(project),
        },
        version,
      ),
    );
  }

  async deleteNode(
    id: string,
    referrers: string[],
    version: string,
    author: string,
  ): Promise<string> {
    const queries: unknown[] = referrers.map((parent) => ({
      "@type": "DeleteTriple",
      subject: { "@type": "NodeValue", node: `Node/${parent}` },
      predicate: { "@type": "NodeValue", node: "depens" },
      object: { "@type": "Value", node: `Node/${id}` },
    }));
    queries.push({
      "@type": "DeleteDocument",
      identifier: { "@type": "NodeValue", node: `Node/${id}` },
    });
    return dataVersion(
      await this.server.request(
        "POST",
        `woql/${this.path}`,
        {},
        {
          commit_info: { author, message: "Delete node" },
          query: { "@type": "And", and: queries },
        },
        version,
      ),
    );
  }

  async history(): Promise<unknown> {
    return (
      await this.server.request("GET", `log/${this.path}`, { count: "50" })
    ).json();
  }

  async createBranch(name: string): Promise<unknown> {
    if (!/^[A-Za-z0-9_-]+$/.test(name))
      throw new DocError("invalid branch name");
    if (
      (await this.server.projects()).some(
        (p) => p.database === this.database && p.branch === name,
      )
    )
      throw new DocError(`branch ${this.database}/${name} already exists`);
    return (
      await this.server.request(
        "POST",
        `branch/admin/${this.database}/local/branch/${name}`,
        {},
        {
          origin: this.path,
        },
      )
    ).json();
  }

  async deleteBranch() {
    await this.server.request("DELETE", `branch/${this.path}`);
  }
  async deleteDatabase() {
    await this.server.request("DELETE", `db/admin/${this.database}`);
  }
}
