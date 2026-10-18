import { randomUUID } from "node:crypto";
import { z } from "zod";
import { Fault } from "./domain.js";
/** LeanGround's explanation of a refusal, such as Lean's messages, is kept up to this length. */
export const MAX_DETAILS = 8000;
export class RemoteError extends Fault {
  constructor(
    code: string,
    public retryable: boolean,
    public remoteStatus: number,
    /** LeanGround's message, e.g. the Lean errors of a rejected proof. */
    public details?: string,
  ) {
    super(code, 502);
  }
}
/** Text cut to MAX_DETAILS characters, marking the cut. */
export function clip(text: string) {
  return text.length > MAX_DETAILS
    ? `${text.slice(0, MAX_DETAILS)}\n… (cut at ${MAX_DETAILS} characters)`
    : text;
}
export async function responseJson(res: Response): Promise<unknown> {
  const text = await res.text();
  if (text.length > 4000000)
    throw new RemoteError("upstream_response_too_large", false, res.status);
  let data: unknown;
  try {
    data = JSON.parse(text);
  } catch {
    throw new RemoteError("invalid_upstream_json", false, res.status);
  }
  if (!res.ok) {
    // A fact error is {kind, reason, error}: `error` explains it (Lean's messages for a proof).
    const code = z
      .object({ reason: z.string().optional(), error: z.string().optional() })
      .passthrough()
      .safeParse(data);
    const reason = code.success
      ? (code.data.reason ?? `upstream_${res.status}`)
      : `upstream_${res.status}`;
    const message = code.success ? code.data.error : undefined;
    throw new RemoteError(
      reason,
      res.status >= 500 || res.status === 429,
      res.status,
      message && message !== reason ? clip(message) : undefined,
    );
  }
  return data;
}
/** Fact operations that run Lean: they may wait for a worker to start or a long check to end. */
const LEAN_OPERATIONS = new Set(["resolve", "submit", "define", "assemble"]);
/** Reads, queries, events and base lookups (docs/mathdoc.md §2.7.7). */
const SHORT_TIMEOUT_MS = 30000;
/** Slightly above LeanGround's own default wall-clock limit per request (900 s). */
export const DEFAULT_LEAN_TIMEOUT_S = 960;
export class LeanGround {
  constructor(
    private url: string,
    private token: string,
    private leanTimeoutMs = DEFAULT_LEAN_TIMEOUT_S * 1000,
  ) {
    const u = new URL(url);
    if (!["http:", "https:"].includes(u.protocol) || u.username || u.password)
      throw new Error("invalid LeanGround URL");
  }
  async call(
    path: string,
    body?: unknown,
    timeoutMs = SHORT_TIMEOUT_MS,
  ): Promise<any> {
    // External payloads are validated at their domain boundary.
    try {
      const res = await fetch(`${this.url.replace(/\/$/, "")}${path}`, {
        method: body === undefined ? "GET" : "POST",
        headers: {
          authorization: `Bearer ${this.token}`,
          "content-type": "application/json",
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(timeoutMs),
        redirect: "error",
      });
      // The time limit covers reading the body too.
      return await responseJson(res);
    } catch (e) {
      if (e instanceof RemoteError) throw e;
      // A timeout may have left work running in LeanGround; jobs retry both.
      throw new RemoteError(
        (e as Error)?.name === "TimeoutError"
          ? "leanground_timeout"
          : "leanground_unavailable",
        true,
        0,
      );
    }
  }
  fact(operation: string, body: unknown) {
    return this.call(
      `/v1/facts/${operation}`,
      body,
      LEAN_OPERATIONS.has(operation) ? this.leanTimeoutMs : SHORT_TIMEOUT_MS,
    );
  }
  /**
   * Module certification (docs/mathdoc.md §4.13): `shared`, `resolve`, `check` and
   * `submit` compile Lean files in a sandbox, so all four get the long time limit.
   */
  modules(operation: "shared" | "resolve" | "check" | "submit", body: unknown) {
    return this.call(`/v1/modules/${operation}`, body, this.leanTimeoutMs);
  }
  /** Submit a project check (docs/external-proofs.md §3.1): Comparator on a registered workspace. */
  projectCheck(body: unknown) {
    return this.call("/v1/project-checks", body);
  }
  /** A project check's state; its result once it has finished. */
  getProjectCheck(id: string) {
    return this.call(`/v1/project-checks/${encodeURIComponent(id)}`);
  }
  /** Check a whole Lean file on these bases: Lean's diagnostics and axioms; no certificate. */
  check(source: string, bases: string[]) {
    return this.call("/v1/check", { source, bases }, this.leanTimeoutMs);
  }
  /** Library declarations whose names contain one of the queries, in this base. */
  search(
    base: Record<string, unknown>,
    queries: string[],
    kind: string,
    limit: number,
  ) {
    return this.call("/v1/retrieval", {
      request_id: randomUUID(),
      base,
      queries,
      kind,
      min_trust: "all",
      limit,
    });
  }
}
