// Document workspace permissions (contract mathdoc.document-permission.v1, decision D).
// A workspace is one TerminusDB database. Roles are never derived from coordination
// projects. A database without a workspace record is visible to administrators only.
import type pg from "pg";
import { z } from "zod";
import { documentPermissionContractSchema } from "../contracts.js";
import { DocError } from "./names.js";

export type Role = "admin" | "owner" | "editor" | "viewer";
export type Action = z.infer<
  typeof documentPermissionContractSchema
>["authorization_matrix"][number]["action"];
type Workspace = z.infer<typeof documentPermissionContractSchema>;

/** The approved v1 matrix (docs/coordination-phase-1-contracts.md §3.4). */
export const APPROVED_MATRIX: Record<Action, Role[]> = {
  read: ["admin", "owner", "editor", "viewer"],
  history: ["admin", "owner", "editor", "viewer"],
  export: ["admin", "owner", "editor", "viewer"],
  write: ["admin", "owner", "editor"],
  branch_create: ["admin", "owner", "editor"],
  proof_request_create: ["admin", "owner", "editor"],
  branch_delete: ["admin", "owner"],
  import: ["admin", "owner"],
  member_manage: ["admin", "owner"],
  database_create: ["admin"],
  database_delete: ["admin"],
};
const RANK: Record<Exclude<Role, "admin">, number> = {
  viewer: 1,
  editor: 2,
  owner: 3,
};

export const WORKSPACE_MIGRATION = `
CREATE TABLE IF NOT EXISTS mdc_workspace(database text PRIMARY KEY, body jsonb NOT NULL);
`;

function matrix() {
  return (Object.keys(APPROVED_MATRIX) as Action[]).map((action) => ({
    action,
    roles: APPROVED_MATRIX[action],
  }));
}

export class Workspaces {
  constructor(
    private pool: pg.Pool,
    private isAdmin: (actor: string) => boolean,
  ) {}

  async get(database: string): Promise<Workspace | null> {
    const row = (
      await this.pool.query(
        "SELECT body FROM mdc_workspace WHERE database=$1",
        [database],
      )
    ).rows[0];
    return row ? documentPermissionContractSchema.parse(row.body) : null;
  }

  role(
    actor: string,
    workspace: Workspace | null,
    branch: string,
  ): Role | null {
    if (this.isAdmin(actor)) return "admin";
    if (!workspace) return null;
    const rule = workspace.branch_rules.find((r) => r.branch === branch);
    let best: Exclude<Role, "admin"> | null = null;
    const consider = (r: Exclude<Role, "admin">) => {
      if (best === null || RANK[r] > RANK[best]) best = r;
    };
    // A branch without a rule is the owner's only (conservative; created branches get a rule).
    if ((rule?.inherit_workspace_role ?? true) && actor === workspace.owner)
      consider("owner");
    for (const g of rule?.grants ?? []) if (g.actor === actor) consider(g.role);
    return best;
  }

  /** Throws 404 when the actor may not even read: no existence leak for others' branches. */
  async authorize(
    actor: string,
    database: string,
    branch: string,
    action: Action,
  ): Promise<Role> {
    const role = this.role(actor, await this.get(database), branch);
    if (!role || !APPROVED_MATRIX.read.includes(role))
      throw new DocError("project not found", 404);
    if (!APPROVED_MATRIX[action].includes(role))
      throw new DocError(
        `${action.replaceAll("_", " ")} requires one of: ${APPROVED_MATRIX[action].join(", ")}`,
        403,
      );
    return role;
  }

  private async save(
    database: string,
    body: Workspace,
    c: pg.PoolClient | pg.Pool = this.pool,
  ) {
    const valid = documentPermissionContractSchema.parse(body);
    await c.query(
      "INSERT INTO mdc_workspace VALUES($1,$2) ON CONFLICT (database) DO UPDATE SET body=EXCLUDED.body",
      [database, valid],
    );
  }

  /** A new database: its creator owns it; main inherits the workspace role. */
  async create(database: string, owner: string) {
    await this.save(database, {
      schema_version: "mathdoc.document-permission.v1",
      workspace_id: database,
      owner,
      branch_rules: [
        { branch: "main", inherit_workspace_role: true, grants: [] },
      ],
      authorization_matrix: matrix(),
    });
  }

  /** Administrators adopt databases created before workspaces existed. */
  async setOwner(database: string, owner: string) {
    const ws = await this.get(database);
    if (ws) await this.save(database, { ...ws, owner });
    else await this.create(database, owner);
  }

  /** A forked branch starts with its origin's rule. */
  async copyRule(database: string, from: string, to: string) {
    const ws = await this.get(database);
    if (!ws) return;
    const origin = ws.branch_rules.find((r) => r.branch === from) ?? {
      branch: from,
      inherit_workspace_role: true,
      grants: [],
    };
    await this.save(database, {
      ...ws,
      branch_rules: [
        ...ws.branch_rules.filter((r) => r.branch !== to),
        { ...origin, branch: to },
      ],
    });
  }

  async dropRule(database: string, branch: string) {
    const ws = await this.get(database);
    if (ws && ws.branch_rules.length > 1)
      await this.save(database, {
        ...ws,
        branch_rules: ws.branch_rules.filter((r) => r.branch !== branch),
      });
  }

  async grant(
    database: string,
    branch: string,
    actor: string,
    role: Exclude<Role, "admin"> | null,
  ) {
    const ws = await this.get(database);
    if (!ws) throw new DocError("project not found", 404);
    const rule = ws.branch_rules.find((r) => r.branch === branch) ?? {
      branch,
      inherit_workspace_role: true,
      grants: [],
    };
    const grants = rule.grants.filter((g) => g.actor !== actor);
    if (role) grants.push({ actor, role });
    await this.save(database, {
      ...ws,
      branch_rules: [
        ...ws.branch_rules.filter((r) => r.branch !== branch),
        { ...rule, grants },
      ],
    });
  }

  async drop(database: string) {
    await this.pool.query("DELETE FROM mdc_workspace WHERE database=$1", [
      database,
    ]);
  }
}
