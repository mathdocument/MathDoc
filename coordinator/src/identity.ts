// Identities and delegated tokens (docs/mathdoc.md §2.7.4). A person's own token (MDC_ACTORS)
// acts as that actor with every right the actor's roles give. A delegated token acts as
// `principal/name`: document roles, membership and ownership are the principal's; leases,
// events, submissions and budgets record the full identity; its scopes limit what it may do.
// Only the SHA-256 of a delegated token is stored; the token itself is shown once.
import {
  createHash,
  randomBytes,
  randomUUID,
  timingSafeEqual,
} from "node:crypto";
import type { IncomingMessage } from "node:http";
import type pg from "pg";
import { z } from "zod";
import { Fault, principalOf, requireThat, type Command } from "./domain.js";

export { principalOf };
/**
 * `work`: read, claim, heartbeat, finish, submit, resolve, note and the like; `maintain`: owner
 * and administrator commands; `edit`: document writes. A person's own token has all three.
 */
export const SCOPES = ["work", "maintain", "edit"] as const;
export type Scope = (typeof SCOPES)[number];
export interface Identity {
  /** `alice`, or `alice/codex-1` for a delegated token. */
  actor: string;
  principal: string;
  /** null for a person's own token: no limit beyond the principal's roles. */
  scopes: Scope[] | null;
}
export function allows(who: Identity, ...needed: Scope[]) {
  return who.scopes === null || needed.every((s) => who.scopes!.includes(s));
}
export function requireScope(who: Identity, ...needed: Scope[]) {
  requireThat(allows(who, ...needed), "token_scope_required", 403);
}

/**
 * What a delegated token needs for each board command. Owner commands need `maintain` (the
 * person must still be the owner); attaching a branch and accepting a writeback change
 * documents, so they also need `edit`.
 */
export const commandScopes: Record<Command["type"], Scope[]> = {
  claim: ["work"],
  heartbeat: ["work"],
  finish: ["work"],
  submit: ["work"],
  submit_modules: ["work"],
  resolve: ["work"],
  note: ["work"],
  edit_note: ["work"],
  delete_note: ["work"],
  negate: ["work"],
  notes: ["work"],
  import: ["work"],
  submit_node: ["work"],
  sync: ["work"],
  attach: ["edit"],
  members: ["maintain"],
  task: ["maintain"],
  decomposition: ["maintain"],
  policy: ["maintain"],
  pick: ["maintain"],
  assemble: ["maintain"],
  retry: ["maintain"],
  accept: ["maintain", "edit"],
};
export const TOKEN_MIGRATION = `
CREATE TABLE IF NOT EXISTS mdc_token(id uuid PRIMARY KEY, principal text NOT NULL, name text NOT NULL, token_sha256 text NOT NULL UNIQUE, scopes text[] NOT NULL, created_at timestamptz NOT NULL DEFAULT now(), expires_at timestamptz, revoked_at timestamptz);
CREATE UNIQUE INDEX IF NOT EXISTS mdc_token_live ON mdc_token(principal, name) WHERE revoked_at IS NULL;
`;
const PREFIX = "mdct_";
const digest = (token: string) =>
  createHash("sha256").update(token, "utf8").digest("hex");
/** Actor names may not contain `/`: it separates a principal from a delegated token's name. */
export const actorName = /^[A-Za-z0-9_-]+$/;

export const tokenInputSchema = z
  .object({
    name: z.string().regex(/^[A-Za-z0-9_-]{1,64}$/),
    scopes: z.array(z.enum(SCOPES)).min(1).max(SCOPES.length).default(["work"]),
    expires_at: z.string().datetime({ offset: true }).optional(),
  })
  .strict();
export type TokenInput = z.input<typeof tokenInputSchema>;

export class Tokens {
  constructor(private pool: pg.Pool) {}
  /** Create a delegated token; the returned `token` is never shown again. */
  async create(principal: string, raw: unknown) {
    const input = tokenInputSchema.parse(raw);
    if (input.expires_at)
      requireThat(
        Date.parse(input.expires_at) > Date.now(),
        "token_expired",
        400,
      );
    const token = `${PREFIX}${randomBytes(32).toString("base64url")}`;
    const scopes = [...new Set(input.scopes)].sort();
    try {
      await this.pool.query(
        "INSERT INTO mdc_token(id,principal,name,token_sha256,scopes,expires_at) VALUES($1,$2,$3,$4,$5,$6)",
        [
          randomUUID(),
          principal,
          input.name,
          digest(token),
          scopes,
          input.expires_at ?? null,
        ],
      );
    } catch (e) {
      if ((e as { code?: string }).code === "23505")
        throw new Fault("token_name_taken");
      throw e;
    }
    return {
      token,
      actor: `${principal}/${input.name}`,
      name: input.name,
      scopes,
      expires_at: input.expires_at ?? null,
    };
  }
  /** The principal's delegated tokens, revoked ones included, without secrets. */
  async list(principal: string) {
    return (
      await this.pool.query(
        `SELECT name, principal||'/'||name AS actor, scopes, created_at, expires_at, revoked_at
           FROM mdc_token WHERE principal=$1 ORDER BY created_at, name`,
        [principal],
      )
    ).rows;
  }
  /** Revoke a live token at once: its next request is unauthorized. */
  async revoke(principal: string, name: string) {
    const row = (
      await this.pool.query(
        `UPDATE mdc_token SET revoked_at=now() WHERE principal=$1 AND name=$2 AND revoked_at IS NULL
           RETURNING name, principal||'/'||name AS actor, revoked_at`,
        [principal, name],
      )
    ).rows[0];
    requireThat(row, "not_found", 404);
    return row;
  }
  async lookup(token: string): Promise<Identity | undefined> {
    if (!token.startsWith(PREFIX)) return undefined;
    const row = (
      await this.pool.query(
        `SELECT principal, name, scopes FROM mdc_token
           WHERE token_sha256=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())`,
        [digest(token)],
      )
    ).rows[0];
    if (!row) return undefined;
    return {
      actor: `${row.principal}/${row.name}`,
      principal: row.principal,
      scopes: row.scopes as Scope[],
    };
  }
}

/**
 * The caller of a request: a person's own token, compared in constant time, or a live delegated
 * token of a principal that is still configured.
 */
export async function identify(
  req: IncomingMessage,
  actors: Record<string, { token: string }>,
  tokens: Tokens,
): Promise<Identity> {
  const token = req.headers.authorization?.replace(/^Bearer /, "") ?? "";
  for (const [actor, entry] of Object.entries(actors)) {
    const a = Buffer.from(token),
      b = Buffer.from(entry.token);
    if (a.length === b.length && timingSafeEqual(a, b))
      return { actor, principal: actor, scopes: null };
  }
  const delegated = await tokens.lookup(token);
  if (delegated && Object.hasOwn(actors, delegated.principal)) return delegated;
  throw new Fault("unauthorized", 401);
}
